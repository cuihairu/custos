//! Network adapter supervision plugin — one plugin process, one resource
//! loop.
//!
//! The enable/disable API is the surface the Windows settings UI itself
//! drives: `netsh interface set interface "<name>" admin=enable|disable`
//! (verified live on the iMac, 2026-11-16 — including the finding that the
//! PowerShell `Enable-NetAdapter`/`Disable-NetAdapter` cmdlets silently do
//! NOTHING on a Hyper-V-equipped host, while netsh works; netsh takes the
//! SetupDi/NDIS path, the cmdlets the CIM path). On Linux the same control
//! surface is `ip link set dev <if> up|down`.
//!
//! SECURITY CONTRACT (docs/guide/plugins.md §5.9):
//! * `network.enable` is a recovery action: an adapter left disabled is the
//!   one fault that cannot be healed over itself — remote access is gone
//!   too. It declares `interrupts_network` + `interrupts_remote_admin`.
//! * `network.disable` is guarded in the plugin, hard-coded:
//!   1. names on `NETADAPTER_PROTECTED` are refused unconditionally,
//!   2. a CONNECTED interface is refused unless `NETADAPTER_ALLOW_ACTIVE=1`
//!      — disabling the adapter that carries the current connection severs
//!      SSH/sunlogin at once (self-lockout).
//!   Every refusal names the reason.
//!
//! Configuration via environment (§4.4 clean env + OS baseline):
//!   NETADAPTER_INTERFACE  interface to supervise (name, or Windows
//!                         ifIndex as `if:19`); default: ANY connected
//!                         non-loopback interface
//!   NETADAPTER_PROTECTED  comma-separated names never allowed to disable
//!   NETADAPTER_ALLOW_ACTIVE  `1` unlocks disabling connected interfaces

use custos_plugin_api::server::{serve, Reply};
use custos_probe::exec::{run_controlled, ExecResult};
use serde_json::{json, Value};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Pure parsing helpers (unit-tested against real-device fixtures)
// ---------------------------------------------------------------------------

/// One adapter row from `netsh interface show interface`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceState {
    pub alias: String,
    pub admin_enabled: bool,
    pub connected: bool,
}

/// The status words netsh prints in the three leading columns (zh-CN and
/// en-US; other locales fall through to the alias column and are skipped —
/// conservative: unknown rows are never acted on).
const NETSH_STATES: [&str; 10] = [
    "已启用",
    "已禁用",
    "Enabled",
    "Disabled",
    "已连接",
    "已断开连接",
    "Connected",
    "Disconnected",
    "专用",
    "Dedicated",
];

/// Parses the whole `netsh interface show interface` table. The three
/// leading cells are state words; everything after them is the interface
/// name (which may contain spaces: `ZeroTier One [..]`, `以太网 2`).
pub fn parse_netsh_interfaces(output: &str) -> Vec<InterfaceState> {
    let mut interfaces = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('-')
            || line.contains("管理员状态")
            || line.contains("Admin State")
        {
            continue;
        }
        let mut admin = None;
        let mut connected = None;
        let mut rest = line;
        // Peel off up to three leading state cells (admin, connection,
        // type); the remainder is the interface name.
        for _ in 0..3 {
            let Some(word) = rest.split_whitespace().next() else {
                break;
            };
            if !NETSH_STATES.contains(&word) {
                break;
            }
            match word {
                "已启用" | "Enabled" => admin = Some(true),
                "已禁用" | "Disabled" => admin = Some(false),
                "已连接" | "Connected" => connected = Some(true),
                "已断开连接" | "Disconnected" => connected = Some(false),
                _ => {}
            }
            // The word starts the line; consume it and the padding after it.
            rest = rest[word.len()..].trim_start();
        }
        let (Some(admin), Some(connected)) = (admin, connected) else {
            continue; // not a data row (e.g. a loopback pseudo-row we don't know)
        };
        let alias = rest.trim().to_owned();
        if alias.is_empty() {
            continue;
        }
        interfaces.push(InterfaceState {
            alias,
            admin_enabled: admin,
            connected,
        });
    }
    interfaces
}

/// One interface row from `ip -o link show` (Linux):
/// `2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ... state UP ...`
pub fn parse_ip_link_row(line: &str) -> Option<InterfaceState> {
    let (_, rest) = line.trim().split_once(':')?;
    let rest = rest.trim_start();
    let (alias, tail) = rest.split_once(':')?;
    // Aliases may carry a virtual-link suffix (`veth0@if12`) — keep the
    // device part.
    let alias = alias.split('@').next().unwrap_or(alias).trim().to_owned();
    let admin_enabled = tail
        .split('<')
        .nth(1)
        .and_then(|flags| flags.split('>').next())
        .map(|flags| flags.split(',').any(|f| f.trim() == "UP"))
        .unwrap_or(false);
    // Operstate column: `state UP` / `state DOWN` / `state UNKNOWN`.
    let connected = tail.contains("state UP");
    Some(InterfaceState {
        alias,
        admin_enabled,
        connected,
    })
}

/// Supervision target: a name, or Windows ifIndex as `if:19`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IfSpec {
    Name(String),
    IfIndex(u32),
}

pub fn parse_if_spec(raw: &str) -> IfSpec {
    if let Some(index) = raw.strip_prefix("if:") {
        if let Ok(n) = index.trim().parse::<u32>() {
            return IfSpec::IfIndex(n);
        }
    }
    IfSpec::Name(raw.trim().to_owned())
}

/// Disable guard — the self-lockout protection, evaluated BEFORE any state
/// change. `state` is None when the interface does not exist.
pub fn disable_allowed(
    alias: &str,
    state: Option<&InterfaceState>,
    protected: &[String],
    allow_active: bool,
) -> Result<(), String> {
    if protected.iter().any(|p| p.eq_ignore_ascii_case(alias)) {
        return Err(format!("interface {alias} is on the protected list (NETADAPTER_PROTECTED)"));
    }
    let Some(state) = state else {
        return Err(format!("interface {alias} not found"));
    };
    if state.connected && !allow_active {
        return Err(format!(
            "refusing to disable CONNECTED interface {alias}: it may be the remote-management path itself \
             (self-lockout); set NETADAPTER_ALLOW_ACTIVE=1 to override"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// I/O layer (cfg-split; non-native branches stay in the pure helpers)
// ---------------------------------------------------------------------------

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn protected_list() -> Vec<String> {
    env("NETADAPTER_PROTECTED")
        .map(|s| s.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect())
        .unwrap_or_default()
}

#[cfg(windows)]
fn list_interfaces() -> Result<Vec<InterfaceState>, String> {
    match run_controlled(
        &["netsh".into(), "interface".into(), "show".into(), "interface".into()],
        Duration::from_secs(15),
        64 * 1024,
    ) {
        ExecResult::Succeeded { stdout, .. } => Ok(parse_netsh_interfaces(&stdout)),
        ExecResult::Failed { code, stderr, .. } => Err(format!("netsh exit {code}: {}", stderr.trim())),
        ExecResult::TimedOut => Err("netsh timed out".into()),
        ExecResult::StartFailed(err) => Err(format!("netsh not runnable: {err}")),
    }
}

#[cfg(not(windows))]
fn list_interfaces() -> Result<Vec<InterfaceState>, String> {
    match run_controlled(&["ip".into(), "-o".into(), "link".into(), "show".into()], Duration::from_secs(10), 64 * 1024) {
        ExecResult::Succeeded { stdout, .. } => Ok(stdout.lines().filter_map(parse_ip_link_row).collect()),
        ExecResult::Failed { code, stderr, .. } => Err(format!("ip exit {code}: {}", stderr.trim())),
        ExecResult::TimedOut => Err("ip timed out".into()),
        ExecResult::StartFailed(err) => Err(format!("ip not runnable: {err}")),
    }
}

/// Resolves an IfSpec::IfIndex to its alias via PowerShell (transport-safe:
/// the numeric index survives any codepage mangling, the alias is fetched
/// locally by the OS itself).
#[cfg(windows)]
fn resolve_alias(spec: &IfSpec) -> Result<String, String> {
    match spec {
        IfSpec::Name(name) => Ok(name.clone()),
        IfSpec::IfIndex(index) => {
            match run_controlled(
                &[
                    "powershell".into(),
                    "-NoProfile".into(),
                    "-Command".into(),
                    format!("(Get-NetAdapter -InterfaceIndex {index}).InterfaceAlias"),
                ],
                Duration::from_secs(30),
                8 * 1024,
            ) {
                ExecResult::Succeeded { stdout, .. } => {
                    let alias = stdout.trim().to_owned();
                    if alias.is_empty() {
                        Err(format!("interface with ifIndex {index} not found"))
                    } else {
                        Ok(alias)
                    }
                }
                ExecResult::Failed { code, stderr, .. } => {
                    Err(format!("cannot resolve ifIndex {index} (exit {code}): {}", stderr.trim()))
                }
                ExecResult::TimedOut => Err(format!("resolving ifIndex {index} timed out")),
                ExecResult::StartFailed(err) => Err(format!("powershell not runnable: {err}")),
            }
        }
    }
}

#[cfg(not(windows))]
fn resolve_alias(spec: &IfSpec) -> Result<String, String> {
    match spec {
        IfSpec::Name(name) => Ok(name.clone()),
        IfSpec::IfIndex(index) => Err(format!("ifIndex addressing is Windows-only (got if:{index})")),
    }
}

/// `netsh interface set interface "<alias>" admin=enable|disable` — the
/// settings-UI surface, NOT the PowerShell cmdlets (silently inert on
/// Hyper-V hosts; caught live on the iMac). The alias goes as a bare argv
/// element: Rust's Windows command-line builder quotes arguments that
/// contain spaces itself — embedding literal quotes corrupts the name
/// (caught live: netsh exit 1, empty stderr).
#[cfg(windows)]
fn set_admin_state(alias: &str, enable: bool) -> Result<(), String> {
    let state = if enable { "enable" } else { "disable" };
    match run_controlled(
        &[
            "netsh".into(),
            "interface".into(),
            "set".into(),
            "interface".into(),
            alias.to_owned(),
            format!("admin={state}"),
        ],
        Duration::from_secs(30),
        8 * 1024,
    ) {
        ExecResult::Succeeded { .. } => Ok(()),
        ExecResult::Failed { code, stderr, .. } => {
            Err(format!("netsh set {state} \"{alias}\" exit {code}: {}", stderr.trim()))
        }
        ExecResult::TimedOut => Err(format!("netsh set {state} timed out")),
        ExecResult::StartFailed(err) => Err(format!("netsh not runnable: {err}")),
    }
}

/// `ip link set dev <if> up|down`.
#[cfg(not(windows))]
fn set_admin_state(alias: &str, enable: bool) -> Result<(), String> {
    let state = if enable { "up" } else { "down" };
    match run_controlled(
        &["ip".into(), "link".into(), "set".into(), "dev".into(), alias.into(), state.into()],
        Duration::from_secs(10),
        8 * 1024,
    ) {
        ExecResult::Succeeded { .. } => Ok(()),
        ExecResult::Failed { code, stderr, .. } => {
            Err(format!("ip link set {alias} {state} exit {code}: {}", stderr.trim()))
        }
        ExecResult::TimedOut => Err(format!("ip link set {state} timed out")),
        ExecResult::StartFailed(err) => Err(format!("ip not runnable: {err}")),
    }
}

/// Enable the given interface; the detail reports the post-change fact.
fn enable_interface(spec: &IfSpec) -> Result<String, String> {
    let alias = resolve_alias(spec)?;
    set_admin_state(&alias, true)?;
    let fact = list_interfaces()?
        .into_iter()
        .find(|i| i.alias == alias)
        .map(|i| {
            if i.connected {
                "connected".to_owned()
            } else {
                "admin-enabled, not connected yet".to_owned()
            }
        })
        .unwrap_or_else(|| "state re-read failed".to_owned());
    Ok(format!("interface {alias} admin-enabled; now: {fact}"))
}

/// Disable the given interface — through the guard.
fn disable_interface(spec: &IfSpec) -> Result<String, String> {
    let alias = resolve_alias(spec)?;
    let state = list_interfaces()?.into_iter().find(|i| i.alias == alias);
    disable_allowed(&alias, state.as_ref(), &protected_list(), env("NETADAPTER_ALLOW_ACTIVE").as_deref() == Some("1"))?;
    set_admin_state(&alias, false)?;
    Ok(format!("interface {alias} admin-disabled"))
}

/// Probe: with `NETADAPTER_INTERFACE` the supervised interface must be
/// admin-enabled AND connected; without it, ANY connected interface is
/// enough (the machine still has a network).
fn probe() -> (bool, String) {
    let interfaces = match list_interfaces() {
        Ok(list) => list,
        Err(err) => return (false, err),
    };
    if interfaces.is_empty() {
        return (false, "no interfaces reported by the OS".into());
    }
    let facts: Vec<String> = interfaces
        .iter()
        .map(|i| {
            let admin = if i.admin_enabled { "enabled" } else { "disabled" };
            let conn = if i.connected { "connected" } else { "disconnected" };
            format!("{}: {admin}/{conn}", i.alias)
        })
        .collect();

    match env("NETADAPTER_INTERFACE").map(|raw| parse_if_spec(&raw)) {
        Some(IfSpec::Name(name)) => {
            let state = interfaces.iter().find(|i| i.alias.eq_ignore_ascii_case(&name));
            let healthy = state.is_some_and(|i| i.admin_enabled && i.connected);
            let verb = match state {
                None => "not found",
                Some(i) if !i.admin_enabled => "admin-disabled",
                Some(i) if !i.connected => "not connected",
                _ => "enabled and connected",
            };
            (healthy, format!("interface {name}: {verb}; {}", facts.join("; ")))
        }
        Some(spec @ IfSpec::IfIndex(_)) => {
            // ifIndex supervision resolves via the alias, best-effort.
            match resolve_alias(&spec) {
                Ok(alias) => {
                    let state = interfaces.iter().find(|i| i.alias.eq_ignore_ascii_case(&alias));
                    let healthy = state.is_some_and(|i| i.admin_enabled && i.connected);
                    let verb = match state {
                        None => "not found",
                        Some(i) if !i.admin_enabled => "admin-disabled",
                        Some(i) if !i.connected => "not connected",
                        _ => "enabled and connected",
                    };
                    (healthy, format!("interface if#{} ({alias}): {verb}; {}", match spec { IfSpec::IfIndex(n) => n.to_string(), _ => String::new() }, facts.join("; ")))
                }
                Err(err) => (false, format!("{err}; {}", facts.join("; "))),
            }
        }
        None => {
            let any_connected = interfaces.iter().any(|i| i.connected);
            (
                any_connected,
                format!(
                    "any-connected policy: {}; {}",
                    if any_connected { "a connected interface exists" } else { "NO connected interface" },
                    facts.join("; ")
                ),
            )
        }
    }
}

fn capabilities() -> Value {
    json!([
        {
            "kind": "probe_provider",
            "operation_id": "network.probe",
            "summary": "adapter inventory with admin/connection state (read-only)",
            "read_only": true,
            "required_permission": "probe.read",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["no_interfaces", "supervised_interface_down"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "network.enable",
            "summary": "admin-enable a network adapter (heals the cannot-be-healed-over-itself fault)",
            "read_only": false,
            "required_permission": "network.manage",
            "affected_scope": "named_resource",
            "interrupts_network": true,
            "interrupts_remote_admin": true,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["interface_not_found", "permission_denied", "state_change_timeout"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "network.disable",
            "summary": "admin-disable a network adapter; refuses connected interfaces (self-lockout guard) and NETADAPTER_PROTECTED names",
            "read_only": false,
            "required_permission": "network.manage",
            "affected_scope": "named_resource",
            "interrupts_network": true,
            "interrupts_remote_admin": true,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["interface_not_found", "protected_interface", "connected_interface_guard", "permission_denied"],
            "compensation_operation": "network.enable"
        }
    ])
}

fn main() {
    serve(|method, params| match method {
        "custos/v1/initialize" => Reply::Result(json!({
            "protocol_version": "1",
            "plugin": "custos-network-adapter-plugin"
        })),
        "custos/v1/describe_capabilities" => Reply::Result(capabilities()),
        "custos/v1/list_recovery_actions" => {
            Reply::Result(json!(["network.enable", "network.disable"]))
        }
        "custos/v1/probe" => {
            let (healthy, observation) = probe();
            Reply::Result(json!({ "healthy": healthy, "observation": observation }))
        }
        "custos/v1/execute_recovery_action" => {
            let operation = params
                .get("operation_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let spec = params
                .get("args")
                .and_then(|args| args.get("interface").and_then(Value::as_str).map(|s| parse_if_spec(s)))
                .or_else(|| {
                    params
                        .get("args")
                        .and_then(|args| args.get("if_index").and_then(Value::as_u64).map(|n| IfSpec::IfIndex(n as u32)))
                })
                .or_else(|| env("NETADAPTER_INTERFACE").map(|raw| parse_if_spec(&raw)));
            let Some(spec) = spec else {
                return Reply::Result(json!({
                    "succeeded": false,
                    "detail": "no target interface: pass args.interface / args.if_index or set NETADAPTER_INTERFACE"
                }));
            };
            let outcome = match operation {
                "network.enable" => enable_interface(&spec),
                "network.disable" => disable_interface(&spec),
                other => return Reply::Error(-32601, format!("unknown operation: {other}")),
            };
            match outcome {
                Ok(detail) => Reply::Result(json!({ "succeeded": true, "detail": detail })),
                Err(err) => Reply::Result(json!({ "succeeded": false, "detail": err })),
            }
        }
        "custos/v1/verify" => {
            let (healthy, observation) = probe();
            Reply::Result(json!({ "healthy": healthy, "observation": observation }))
        }
        "custos/v1/shutdown" => Reply::Exit(json!({ "ok": true })),
        other => Reply::Error(-32601, format!("unknown method: {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REAL iMac output, 2026-11-16 (zh-CN Windows 11, `netsh interface
    /// show interface`), aliases include spaces and brackets.
    const IMAC_NETSH_ZH: &str = "\
管理员状态     状态           类型             接口名称
-------------------------------------------------------------------------
已启用            已连接            专用               以太网
已启用            已连接            专用               ZeroTier One [db64858fedb29f06]
已启用            已断开连接          专用               WLAN
已启用            已断开连接          专用               以太网 2
已启用            已断开连接          专用               WLAN 2
已启用            已连接            专用               wg-meshlet
";

    #[test]
    fn netsh_zh_fixture_rows() {
        let list = parse_netsh_interfaces(IMAC_NETSH_ZH);
        assert_eq!(list.len(), 6, "{list:?}");
        let ethernet = &list[0];
        assert_eq!(ethernet.alias, "以太网");
        assert!(ethernet.admin_enabled && ethernet.connected);
        let zerotier = list.iter().find(|i| i.alias.contains("ZeroTier")).expect("zerotier");
        assert_eq!(zerotier.alias, "ZeroTier One [db64858fedb29f06]");
        assert!(zerotier.connected);
        let wlan = list.iter().find(|i| i.alias == "WLAN").expect("wlan");
        assert!(!wlan.connected);
        let ethernet2 = list.iter().find(|i| i.alias == "以太网 2").expect("eth2");
        assert!(!ethernet2.connected);
    }

    #[test]
    fn netsh_en_fixture_rows() {
        let output = "\
Admin State    State          Type             Interface Name
-------------------------------------------------------------------------
Enabled        Connected      Dedicated        Ethernet
Enabled        Disconnected   Dedicated        Local Area Connection* 1
Disabled       Disconnected   Dedicated        Wi-Fi
";
        let list = parse_netsh_interfaces(output);
        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(list[0].alias, "Ethernet");
        assert!(list[0].admin_enabled && list[0].connected);
        assert_eq!(list[1].alias, "Local Area Connection* 1");
        assert!(list[1].admin_enabled && !list[1].connected);
        assert!(!list[2].admin_enabled);
    }

    #[test]
    fn ip_link_fixture_rows() {
        let output = "\
1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000
2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP mode DEFAULT group default qlen 1000
3: wlan0: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 qdisc noqueue state DOWN mode DEFAULT group default qlen 1000
4: wg0: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1420 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000
";
        let rows: Vec<InterfaceState> = output.lines().filter_map(parse_ip_link_row).collect();
        assert_eq!(rows.len(), 4);
        let eth0 = &rows[1];
        assert_eq!(eth0.alias, "eth0");
        assert!(eth0.admin_enabled && eth0.connected);
        let wlan0 = &rows[2];
        assert!(wlan0.admin_enabled, "UP flag set");
        assert!(!wlan0.connected, "operstate DOWN");
        let lo = &rows[0];
        assert!(!lo.connected);
    }

    #[test]
    fn if_spec_parsing() {
        assert_eq!(parse_if_spec("if:19"), IfSpec::IfIndex(19));
        assert_eq!(parse_if_spec(" 以太网 2 "), IfSpec::Name("以太网 2".into()));
        assert_eq!(parse_if_spec("if:abc"), IfSpec::Name("if:abc".into()));
    }

    #[test]
    fn disable_guard_refuses_connected_and_protected() {
        let connected = InterfaceState {
            alias: "以太网".into(),
            admin_enabled: true,
            connected: true,
        };
        let idle = InterfaceState {
            alias: "以太网 2".into(),
            admin_enabled: true,
            connected: false,
        };
        // Connected interface: refused, reason names the self-lockout risk.
        let err = disable_allowed("以太网", Some(&connected), &[], false).unwrap_err();
        assert!(err.contains("CONNECTED") && err.contains("self-lockout"), "{err}");
        // Explicit override unlocks it.
        assert!(disable_allowed("以太网", Some(&connected), &[], true).is_ok());
        // Idle interface: fine.
        assert!(disable_allowed("以太网 2", Some(&idle), &[], false).is_ok());
        // Protected list wins over everything, case-insensitive.
        let protected = vec!["以太网".into(), "wan0".into()];
        let err = disable_allowed("以太网", Some(&connected), &protected, true).unwrap_err();
        assert!(err.contains("protected"), "{err}");
        assert!(disable_allowed("WAN0", Some(&idle), &protected, false).is_err());
        // Missing interface: refused with the name.
        let err = disable_allowed("eth9", None, &[], false).unwrap_err();
        assert!(err.contains("eth9"), "{err}");
    }
}

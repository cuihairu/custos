//! Windows adapters (docs/guide/platforms.md §11.2). The command builders
//! and output parsers are pure and unit-tested on every platform; the
//! execution wrapper is `cfg(windows)`. Every dangerous operation declares
//! its risk; the permission gate lives in the dispatch layer (ADR-0012).

use crate::capability::PlatformCapabilitySet;
use crate::controller::{
    Connectivity, ConnectivityTarget, CredentialProvider, InterfaceStatus,
    NetworkConnectivityProbe, NetworkInterfaceController, ProcessController, ProcessInfo,
    RebootController, ServiceController, ServiceState,
};
use crate::error::{PlatformError, PlatformResult};
#[cfg(windows)]
use custos_probe::exec::run_controlled;
#[cfg(windows)]
use custos_probe::exec::ExecResult;
use std::time::Duration;

const TOOL_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(windows)]
const TOOL_OUTPUT_CAP: usize = 64 * 1024;

#[cfg(windows)]
fn run(argv: &[&str]) -> ExecResult {
    let owned: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
    run_controlled(&owned, TOOL_TIMEOUT, TOOL_OUTPUT_CAP)
}

/// Ok when the controlled execution succeeded; the unified error otherwise.
#[cfg(windows)]
fn expect_success(result: ExecResult) -> PlatformResult<()> {
    match result {
        ExecResult::Succeeded { .. } => Ok(()),
        other => Err(other.into()),
    }
}

/// Builds the `tasklist /FI "PID eq <pid>"` argv (pure; execution is
/// cfg(windows)). Presence of the pid line decides liveness.
pub fn tasklist_argv(pid: u32) -> Vec<String> {
    vec![
        "tasklist".into(),
        "/FI".into(),
        format!("PID eq {pid}"),
        "/NH".into(),
        "/FO".into(),
        "CSV".into(),
    ]
}

/// True when tasklist CSV output actually lists the pid (not `INFO:`).
pub fn tasklist_has_pid(output: &str, pid: u32) -> bool {
    output
        .lines()
        .any(|line| line.starts_with('"') && line.contains(&format!("\"{pid}\"")))
}

/// Windows process detection via `tasklist` (read-only).
pub struct WindowsProcessController;

impl ProcessController for WindowsProcessController {
    #[cfg(windows)]
    fn is_alive(&self, pid: u32) -> bool {
        match run(&tasklist_argv(pid)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>())
        {
            ExecResult::Succeeded { stdout, .. } => tasklist_has_pid(&stdout, pid),
            _ => false,
        }
    }

    #[cfg(not(windows))]
    fn is_alive(&self, _pid: u32) -> bool {
        false // tasklist does not exist on this host; do not fake support
    }

    #[cfg(windows)]
    fn find_by_name(&self, name: &str) -> PlatformResult<Vec<ProcessInfo>> {
        let argv = ["tasklist", "/NH", "/FO", "CSV"];
        match run(&argv) {
            ExecResult::Succeeded { stdout, .. } => Ok(parse_tasklist_csv(&stdout)
                .into_iter()
                .filter(|p| p.name == name)
                .collect()),
            other => Err(other.into()),
        }
    }

    #[cfg(not(windows))]
    fn find_by_name(&self, _name: &str) -> PlatformResult<Vec<ProcessInfo>> {
        Err(PlatformError::Unsupported(
            "tasklist does not exist on this platform".into(),
        ))
    }
}

/// Parses `tasklist /NH /FO CSV` rows: `"name","pid","session",...`.
pub fn parse_tasklist_csv(output: &str) -> Vec<ProcessInfo> {
    let mut processes = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if !line.starts_with('"') {
            continue; // header or "INFO: no tasks" note
        }
        let mut fields = line.split("\",\"");
        let Some(name) = fields.next().map(|f| f.trim_start_matches('"')) else {
            continue;
        };
        let Some(pid) = fields
            .next()
            .and_then(|f| f.trim_end_matches('"').parse::<u32>().ok())
        else {
            continue;
        };
        processes.push(ProcessInfo {
            pid,
            name: name.to_owned(),
        });
    }
    processes
}

/// Builds the service-control argv per verb (pure).
pub fn sc_argv(verb: &str, service: &str) -> Vec<String> {
    match verb {
        "status" => vec!["sc".into(), "query".into(), service.into()],
        "start" => vec!["sc".into(), "start".into(), service.into()],
        "stop" => vec!["sc".into(), "stop".into(), service.into()],
        _ => vec!["sc".into(), verb.into(), service.into()],
    }
}

/// Maps `sc query` output to the normalized service state. The `STATE`
/// line is indented, so match on the label before the colon.
pub fn parse_sc_query(output: &str) -> ServiceState {
    for line in output.lines() {
        let trimmed = line.trim();
        let Some((label, rest)) = trimmed.split_once(':') else {
            continue;
        };
        if !label.trim().starts_with("STATE") {
            continue;
        }
        // `sc query` prints `STATE : <code>  <NAME>` — accept either.
        let code = rest.split_whitespace().next().unwrap_or("");
        return match code {
            "4" | "RUNNING" => ServiceState::Running,
            "1" | "STOPPED" => ServiceState::Stopped,
            "2" | "3" | "6" | "7" | "START_PENDING" | "STOP_PENDING" | "PAUSE_PENDING"
            | "CONTINUE_PENDING" => ServiceState::Transitioning,
            "5" | "PAUSED" => ServiceState::Unknown("paused".into()),
            other => ServiceState::Unknown(other.to_owned()),
        };
    }
    ServiceState::Unknown(output.trim().to_owned())
}

/// Windows SCM service control via the `windows-service` crate
/// (todo.md 194 matrix: ServiceManager start/stop/status with a settle
/// timeout and result verification).
pub struct WindowsScmController;

impl ServiceController for WindowsScmController {
    fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
        #[cfg(windows)]
        return crate::init_systems::scm::ScmController.status(unit);
        #[cfg(not(windows))]
        {
            let _ = unit;
            Err(PlatformError::Unsupported(
                "the windows-service SCM path does not exist on this platform".into(),
            ))
        }
    }
    fn start(&self, _unit: &str) -> PlatformResult<()> {
        #[cfg(windows)]
        return crate::init_systems::scm::ScmController.start(_unit);
        #[cfg(not(windows))]
        refuse()
    }
    fn stop(&self, _unit: &str) -> PlatformResult<()> {
        #[cfg(windows)]
        return crate::init_systems::scm::ScmController.stop(_unit);
        #[cfg(not(windows))]
        refuse()
    }
    fn restart(&self, _unit: &str) -> PlatformResult<()> {
        #[cfg(windows)]
        return crate::init_systems::scm::ScmController.restart(_unit);
        #[cfg(not(windows))]
        refuse()
    }
}

#[cfg(not(windows))]
fn refuse() -> PlatformResult<()> {
    Err(PlatformError::Unsupported(
        "the windows-service SCM path does not exist on this platform".into(),
    ))
}

/// Windows SCM service control via `sc` (controlled execution); kept as
/// the fallback path when the SCM crate path is unavailable.
pub struct ScServiceController;

impl ServiceController for ScServiceController {
    fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
        #[cfg(windows)]
        {
            let argv = sc_argv("status", unit);
            match run(&argv.iter().map(String::as_str).collect::<Vec<_>>()) {
                ExecResult::Succeeded { stdout, .. } => Ok(parse_sc_query(&stdout)),
                ExecResult::Failed { code, stderr, .. } if code == 1060 => {
                    Err(PlatformError::NotFound(format!(
                        "service {unit} not installed ({})",
                        stderr.trim()
                    )))
                }
                other => Err(other.into()),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = unit;
            Err(PlatformError::Unsupported(
                "sc.exe does not exist on this platform".into(),
            ))
        }
    }

    fn start(&self, unit: &str) -> PlatformResult<()> {
        #[cfg(windows)]
        {
            let argv = sc_argv("start", unit);
            expect_success(run(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
        }
        #[cfg(not(windows))]
        {
            let _ = unit;
            Err(PlatformError::Unsupported(
                "sc.exe does not exist on this platform".into(),
            ))
        }
    }

    fn stop(&self, unit: &str) -> PlatformResult<()> {
        #[cfg(windows)]
        {
            let argv = sc_argv("stop", unit);
            expect_success(run(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
        }
        #[cfg(not(windows))]
        {
            let _ = unit;
            Err(PlatformError::Unsupported(
                "sc.exe does not exist on this platform".into(),
            ))
        }
    }

    fn restart(&self, unit: &str) -> PlatformResult<()> {
        self.stop(unit)?;
        self.start(unit)
    }
}

/// Builds the default gateway check argv (pure): `route print -4` lists
/// `0.0.0.0  0.0.0.0  <gateway> ...` for the default route.
pub fn route_print_argv() -> Vec<String> {
    vec!["route".into(), "print".into(), "-4".into()]
}

/// Extracts the default gateway from `route print -4` output: the row whose
/// network destination and mask are both `0.0.0.0`. Stops at the localized
/// Persistent Routes section header so a stale persistent default route is
/// never reported as the active gateway (openclaw audit 2026-11-05) —
/// mirrors `parse_route_print_default_gateway` in custos-probe/netif.rs.
pub fn parse_default_gateway(output: &str) -> Option<String> {
    for line in output.lines() {
        // Mirrors the netif.rs scanner: the Persistent Routes section keeps
        // the configured gateway after the active route drops; scanning
        // past its header would report that stale route as Healthy.
        if custos_probe::netif::is_persistent_routes_header(line) {
            break;
        }
        let mut fields = line.split_whitespace();
        let Some(destination) = fields.next() else {
            continue;
        };
        if destination != "0.0.0.0" {
            continue;
        }
        let Some(mask) = fields.next() else {
            continue;
        };
        if mask != "0.0.0.0" {
            continue;
        }
        if let Some(gateway) = fields.next() {
            if gateway != "On-link" && !gateway.is_empty() {
                return Some(gateway.to_owned());
            }
        }
    }
    None
}

/// Parses `netsh wlan show interfaces` output for the connected SSID and
/// state. Pure, fixture-testable.
pub fn parse_netsh_wlan_state(output: &str) -> (bool, Option<String>) {
    let mut connected = false;
    let mut ssid = None;
    for line in output.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim();
        let value = value.trim();
        if label.starts_with("State") || label.starts_with("状态") {
            connected = value.contains("connected") || value.contains("已连接");
        } else if label.starts_with("SSID") && !label.contains("BSSID") && !value.is_empty() {
            ssid = Some(value.to_owned());
        }
    }
    (connected, ssid)
}

/// Builds the Wi-Fi reconnect argv for an SSID and profile (pure).
///
/// UNWIRED (openclaw audit 2026-11-05): no dispatch path executes this
/// argv — the [`NetshNetworkController::connect_wifi`] execution path and
/// the [`has_wifi_adapter`] adapter probe now wire it, and the static
/// [`capabilities`] set still lists `wifi.reconnect` as unsupported: the
/// runtime gate is the adapter probe, not the static declaration (the
/// static set cannot call the OS at declaration time).
pub fn netsh_wlan_connect_argv(profile: &str) -> Vec<String> {
    vec![
        "netsh".into(),
        "wlan".into(),
        "connect".into(),
        format!("name={profile}"),
    ]
}

/// Detects a usable wireless adapter in `netsh wlan show interfaces`
/// output. The command exits 0 BOTH when an adapter exists and when the
/// system reports "There is no wireless interface on the system." — the
/// exit status alone is not evidence of a radio (openclaw audit
/// 2026-11-05). Pure, fixture-testable on every platform.
pub fn netsh_wlan_reports_adapter(output: &str) -> bool {
    for line in output.lines() {
        let line = line.trim();
        // Localized "no wireless interface" markers. `netsh wlan show
        // interfaces` prints one of these when the host has no radio; the
        // exit status alone is not evidence either way (openclaw audit
        // 2026-11-05). Covers en/zh-CN plus the other common Windows
        // locales; a marker that still fails to match leaves the default
        // `false` (conservative: no adapter is never claimed native).
        if is_no_wlan_marker(line) {
            return false;
        }
        // An interface block carries a `Name : <value>` field.
        if let Some((label, value)) = line.split_once(':') {
            let label = label.trim();
            if is_wlan_name_label(label) && !value.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

/// True when `line` is a localized "no wireless interface on the system"
/// notice. `route`/`netsh` localize these strings; the marker set mirrors
/// the Windows locales the route header parser covers. Pure.
fn is_no_wlan_marker(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("no wireless interface")
        || lower.contains("no wireless adapter")
        || line.contains("没有无线接口")
        || line.contains("ワイヤレス インターフェイスがシステム上にありません")
        || lower.contains("aucune interface sans fil")
        || lower.contains("keine drahtlose schnittstelle")
        || lower.contains("keine drahtlose schnittstelle vorhanden")
        || line.contains("무선 인터페이스가 없습니다")
        || lower.contains("беспроводной интерфейс не найден")
}

/// True when `label` is a localized `Name` field label in a
/// `netsh wlan show interfaces` block. Non-English Windows localize the
/// column (ja 名前 / fr Nom / ko 이름 / ru Имя / es Nombre / it Nome /
/// pl Nazwa / nl Naam / tr Ad); matching only `Name`/`名称` would report
/// no adapter on those systems (openclaw audit 2026-11-05). Pure.
fn is_wlan_name_label(label: &str) -> bool {
    matches!(
        label,
        "Name" | "名称" | "名前" | "Nom" | "이름" | "Имя" | "Nombre" | "Nome" | "Nazwa"
            | "Naam" | "Ad" | "Název"
    )
}

/// True when the host has a usable Wi-Fi adapter — probed via
/// `netsh wlan show interfaces` and confirmed by the interface list, never
/// guessed. Windows-only; other platforms always answer false because
/// netsh does not exist there.
pub fn has_wifi_adapter() -> bool {
    #[cfg(windows)]
    {
        let argv = ["netsh", "wlan", "show", "interfaces"];
        match run(&argv) {
            ExecResult::Succeeded { stdout, .. } => netsh_wlan_reports_adapter(&stdout),
            _ => false,
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Builds the interface reset argv (HIGH RISK: may cut the management
/// channel — callers must gate and mark interrupts_remote_admin).
pub fn netsh_interface_reset_argv(name: &str) -> Vec<String> {
    vec![
        "netsh".into(),
        "interface".into(),
        "set".into(),
        "interface".into(),
        format!("name={name}"),
        "admin=disabled".into(),
        "admin=enabled".into(),
    ]
}

/// Builds the DNS flush argv (pure).
pub fn ipconfig_flushdns_argv() -> Vec<String> {
    vec!["ipconfig".into(), "/flushdns".into()]
}

/// Windows network operations via `netsh`/`ipconfig`/`route`.
pub struct NetshNetworkController;

impl NetworkInterfaceController for NetshNetworkController {
    fn list(&self) -> PlatformResult<Vec<InterfaceStatus>> {
        #[cfg(windows)]
        {
            let argv = ["netsh", "interface", "show", "interface"];
            match run(&argv) {
                ExecResult::Succeeded { stdout, .. } => Ok(parse_netsh_interfaces(&stdout)),
                other => Err(other.into()),
            }
        }
        #[cfg(not(windows))]
        {
            Err(PlatformError::Unsupported(
                "netsh.exe does not exist on this platform".into(),
            ))
        }
    }

    fn reset(&self, name: &str) -> PlatformResult<()> {
        #[cfg(windows)]
        {
            let argv = netsh_interface_reset_argv(name);
            expect_success(run(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
        }
        #[cfg(not(windows))]
        {
            let _ = name;
            Err(PlatformError::Unsupported(
                "netsh.exe does not exist on this platform".into(),
            ))
        }
    }

    fn connect_wifi(&self, profile: &str) -> PlatformResult<()> {
        #[cfg(windows)]
        {
            if !has_wifi_adapter() {
                return Err(PlatformError::Unsupported(
                    "no Wi-Fi adapter detected via `netsh wlan show interfaces`".into(),
                ));
            }
            let argv = netsh_wlan_connect_argv(profile);
            expect_success(run(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
        }
        #[cfg(not(windows))]
        {
            let _ = profile;
            Err(PlatformError::Unsupported(
                "netsh.exe does not exist on this platform".into(),
            ))
        }
    }
}

/// Parses `netsh interface show interface` rows:
/// `Admin State  State  Type  Interface Name` — enabled + connected = up.
pub fn parse_netsh_interfaces(output: &str) -> Vec<InterfaceStatus> {
    let mut interfaces = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Admin State")
            || trimmed.starts_with("管理员状态")
            || trimmed.starts_with('-')
            || trimmed.contains("Loopback")
        {
            continue;
        }
        let admin_enabled = trimmed.contains("Enabled") || trimmed.contains("已启用");
        let connected = trimmed.contains("Connected") || trimmed.contains("已连接");
        let name = match trimmed.rsplit_once("  ") {
            Some((_left, name)) => name.trim().to_owned(),
            None => continue,
        };
        interfaces.push(InterfaceStatus {
            name,
            up: admin_enabled && connected,
            addresses: Vec::new(),
        });
    }
    interfaces
}

impl NetworkConnectivityProbe for NetshNetworkController {
    fn probe_tcp(&self, target: &ConnectivityTarget) -> PlatformResult<Connectivity> {
        use std::net::TcpStream;
        let start = std::time::Instant::now();
        match TcpStream::connect_timeout(
            &format!("{}:{}", target.host, target.port)
                .parse()
                .map_err(|_| PlatformError::Failed("invalid target".into()))?,
            TOOL_TIMEOUT,
        ) {
            Ok(_) => Ok(Connectivity::Reachable {
                latency_ms: start.elapsed().as_millis() as u64,
            }),
            Err(err) => match err.kind() {
                std::io::ErrorKind::ConnectionRefused => Ok(Connectivity::Refused),
                _ => Ok(Connectivity::Unreachable(err.to_string())),
            },
        }
    }
}

/// Windows credential reads from the environment (P2 baseline; DPAPI
/// keyring comes later). Values are never logged by callers.
pub struct EnvCredentialProvider;

impl CredentialProvider for EnvCredentialProvider {
    fn get(&self, name: &str) -> PlatformResult<Option<String>> {
        Ok(std::env::var(name).ok())
    }
}

/// Authorized reboot: `shutdown /r /t 0`. Gated at dispatch (ADR-0012).
pub struct WindowsRebootController;

impl RebootController for WindowsRebootController {
    fn reboot(&self) -> PlatformResult<()> {
        #[cfg(windows)]
        {
            let argv = ["shutdown", "/r", "/t", "0"];
            expect_success(run(&argv))
        }
        #[cfg(not(windows))]
        {
            Err(PlatformError::Unsupported(
                "shutdown.exe does not exist on this platform".into(),
            ))
        }
    }
}

/// The Windows adapter's declared support (todo.md 193). Declares only
/// operations with a real execution path behind them (§11.1: 不得伪造平台
/// 支持). `wifi.reconnect` is declared native only when the runtime
/// [`has_wifi_adapter`] probe sees a real Wi-Fi adapter AND the
/// [`NetshNetworkController::connect_wifi`] execution path exists; on a
/// non-Windows host the probe answers false and the declaration stays
/// unsupported (never guessed).
pub fn capabilities() -> PlatformCapabilitySet {
    let mut set = PlatformCapabilitySet::new()
        .native("process.detect")
        .native("service.status")
        .native("service.start")
        .native("service.stop")
        .native("service.restart")
        .native("network.interface.list")
        .native("network.interface.reset")
        .native("network.connectivity.tcp");
    if has_wifi_adapter() {
        // Wired: the adapter probe saw a real radio and connect_wifi runs
        // `netsh wlan connect` behind it (openclaw audit 2026-11-05).
        set = set.native("wifi.reconnect");
    } else {
        set = set.unsupported(
            "wifi.reconnect",
            "no Wi-Fi adapter detected via `netsh wlan show interfaces`",
        );
    }
    set.native("dns.flush").native("system.reboot")
}

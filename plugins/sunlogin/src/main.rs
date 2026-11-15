//! Sunlogin (向日葵/AweSun) plugin — one plugin process, one resource loop.
//!
//! 向日葵 is closed-source with no official API. The wrapper follows the
//! clash-api principle: the de-facto API is whatever surface the product's
//! own UI talks to. Two generations exist in the wild:
//!
//! * Legacy SunloginClient (11.x era): loopback HTTP status endpoint
//!   (`/cgi-bin/rpc?action=login-type` → version/binding) on a random port
//!   written into the client logs. This surface is OPTIONAL here: enable it
//!   with `SUNLOGIN_API` / `SUNLOGIN_PORT` on installs where it works.
//! * AweSun 16.x (verified live on the iMac, 2026-11-16): the HTTP surface
//!   is GONE — the loopback listeners are the GUI↔service private IPC. The
//!   observable health model is therefore service + processes + cloud:
//!     1. `AweSunService` RUNNING          (`sc query`)
//!     2. client processes alive           (`tasklist`: AweSun.exe, awesun_guard)
//!     3. cloud session ESTABLISHED        (`netstat -ano`: client pid → :443)
//!     4. config.ini                       (full_version, account = binding)
//!   "cloud session up" is THE health signal: the machine is remotely
//!   controllable from outside right now.
//!
//! SECURITY CONTRACT (docs/guide/plugins.md §5.8): read-only observation
//! only. The plugin never touches historical command endpoints (`/check` —
//! the CNVD-2022-10270 RCE surface, fixed upstream).
//!
//! Configuration via environment (§4.4 clean env):
//!   SUNLOGIN_SERVICE   service name/unit — pins exactly ONE candidate,
//!                      no fallbacks (Windows default AweSunService;
//!                      Linux default sunloginclient)
//!   SUNLOGIN_CONFIG    client config.ini override (version/binding facts)
//!   SUNLOGIN_API       legacy loopback status base, e.g.
//!                      http://127.0.0.1:49181 (optional enrichment)
//!   SUNLOGIN_PORT      legacy port override (optional enrichment)
//!   SUNLOGIN_LOG_DIR   legacy client log directory override (optional)

use custos_plugin_api::server::{serve, Reply};
use custos_probe::exec::{run_controlled, ExecResult};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;

/// Plausible local listening ports for the legacy HTTP surface.
const PORT_MIN: u16 = 40_000;
const PORT_MAX: u16 = 65_535;

/// Client process-name fragments (lowercase `contains` match): the AweSun
/// international build and the 中文 SunloginClient build.
const PROCESS_MARKERS: [&str; 2] = ["awesun", "sunlogin"];

// ---------------------------------------------------------------------------
// Pure parsing helpers (unit-tested against real-device fixtures)
// ---------------------------------------------------------------------------

/// One client process: normalized (lowercased) name + pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub name: String,
    pub pid: u32,
}

impl ProcessEntry {
    fn is_client(&self) -> bool {
        PROCESS_MARKERS.iter().any(|m| self.name.contains(m))
    }
}

/// A discovered process counts as the client only when it is a client
/// marker process AND NOT the plugin itself: not our own pid, and not any
/// sibling instance sharing our executable name. The iMac live run caught
/// the sibling case — two concurrent plugin processes (parallel live
/// tests, or engine + manual session) must not vouch for each other's
/// client presence.
fn is_client_process(p: &ProcessEntry, own_pid: u32, own_name: Option<&str>) -> bool {
    p.is_client() && p.pid != own_pid && own_name != Some(p.name.as_str())
}

/// Own process identity for the self-exclusion above.
fn own_process_identity() -> (u32, Option<String>) {
    let own_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    (std::process::id(), own_name)
}

/// Observed service state. Transition states (STOP_PENDING/START_PENDING)
/// are their own state — the iMac live run showed treating them as
/// "not installed" produces a false fact AND a falsely-hasty restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Stopped,
    Pending,
    Missing,
}

/// `sc query {name}` → the service's state. `sc query` on a missing service
/// prints no STATE line at all ("The specified service does not exist").
pub fn parse_sc_query(text: &str) -> ServiceState {
    for line in text.lines() {
        if !line.contains("STATE") {
            continue;
        }
        if line.contains("RUNNING") {
            return ServiceState::Running;
        }
        if line.contains("_PENDING") || line.contains("PAUSED") {
            return ServiceState::Pending;
        }
        if line.contains("STOPPED") {
            return ServiceState::Stopped;
        }
    }
    ServiceState::Missing
}

/// One `tasklist /FO CSV /NH` row: `"AweSun.exe","5124","Services","0",".."`
pub fn parse_tasklist_line(line: &str) -> Option<ProcessEntry> {
    let mut fields = line.split(',');
    let name = fields.next()?.trim_matches('"');
    let pid = fields.next()?.trim_matches('"').parse::<u32>().ok()?;
    Some(ProcessEntry {
        name: name.to_lowercase(),
        pid,
    })
}

/// One `ps -eo pid=,comm=` row: `5124 awesun`
pub fn parse_ps_line(line: &str) -> Option<ProcessEntry> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse::<u32>().ok()?;
    let name = fields.next()?.to_lowercase();
    Some(ProcessEntry { name, pid })
}

/// True when a `netstat -ano` line is an ESTABLISHED :443 session owned by
/// one of `pids` — the client's cloud session (the remotely-controllable
/// signal). Column order: Proto, Local, Remote, State, PID.
pub fn netstat_cloud_session(line: &str, pids: &HashSet<u32>) -> bool {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() != 5 || !fields[3].contains("ESTABLISHED") {
        return false;
    }
    if !fields[2].rsplit_once(':').map(|(_, p)| p == "443").unwrap_or(false) {
        return false;
    }
    fields[4].parse::<u32>().map(|pid| pids.contains(&pid)).unwrap_or(false)
}

/// True when an `ss -tnp` line is an ESTABLISHED :443 session whose process
/// is a sunlogin/awesun client: `ESTAB 0 0 1.2.3.4:5 47.98.1.2:443
/// users:(("sunloginclient",pid=5124))`
pub fn ss_cloud_session(line: &str) -> bool {
    if !line.contains("ESTAB") {
        return false;
    }
    let remote_has_443 = line
        .split_whitespace()
        .any(|t| t.rsplit_once(':').map(|(_, p)| p == "443").unwrap_or(false));
    if !remote_has_443 {
        return false;
    }
    PROCESS_MARKERS.iter().any(|m| line.contains(m))
}

/// Facts read from the client's `config.ini` (key=value ini, values may be
/// encrypted — only the plain keys are read).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientConfig {
    pub version: String,
    pub account: String,
}

/// Parse `full_version` / `account` out of a config.ini text.
pub fn parse_client_config(text: &str) -> ClientConfig {
    let value = |key: &str| {
        text.lines()
            .find(|l| l.starts_with(key) && l[key.len()..].starts_with('='))
            .map(|l| l[key.len() + 1..].trim().to_owned())
            .unwrap_or_default()
    };
    ClientConfig {
        version: value("full_version"),
        account: value("account"),
    }
}

/// Scan a legacy log text for the client's local listening port (old
/// versions wrote the random high port into startup lines).
pub fn find_port_in_log(text: &str) -> Option<u16> {
    for line in text.lines() {
        let lower = line.to_lowercase();
        if !(lower.contains("listen") || lower.contains("port") || lower.contains("http")) {
            continue;
        }
        for token in lower.split(|c: char| !c.is_ascii_digit()) {
            if let Ok(port) = token.parse::<u16>() {
                if (PORT_MIN..=PORT_MAX).contains(&port) {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// One parsed legacy `login-type` answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientStatus {
    pub version: String,
    pub bound: bool,
    pub platform: String,
    pub mac: String,
}

/// Lenient parse of `/cgi-bin/rpc?action=login-type` (legacy versions).
pub fn parse_login_type(value: &Value) -> ClientStatus {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    ClientStatus {
        version: text("version"),
        bound: value.get("isbinding").and_then(Value::as_str) == Some("1"),
        platform: text("platform"),
        mac: text("mac"),
    }
}

// ---------------------------------------------------------------------------
// I/O layer: per-platform observation (all compile-checked; the non-native
// branches stay in pure helpers and are exercised by fixtures)
// ---------------------------------------------------------------------------

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// Service names/units to try. `SUNLOGIN_SERVICE` PINS exactly one candidate
/// (no fallbacks: a wrong fallback must never be restarted); without the
/// env, the platform default plus its alternate names are tried in order.
fn service_candidates() -> Vec<String> {
    if let Some(pinned) = env("SUNLOGIN_SERVICE") {
        return vec![pinned];
    }
    let names: [&str; 3] = if cfg!(windows) {
        ["AweSunService", "SunloginService", "AweSun Service"]
    } else {
        ["sunloginclient", "oray-sunlogin", "awesun"]
    };
    names.iter().map(|n| (*n).to_owned()).collect()
}

#[cfg(windows)]
fn service_state(name: &str) -> Result<ServiceState, String> {
    match run_controlled(&["sc".into(), "query".into(), name.into()], Duration::from_secs(10), 8 * 1024) {
        ExecResult::Succeeded { stdout, .. } => Ok(parse_sc_query(&stdout)),
        ExecResult::Failed { stdout, .. } => Ok(parse_sc_query(&stdout)),
        ExecResult::TimedOut => Err(format!("sc query {name} timed out")),
        ExecResult::StartFailed(err) => Err(format!("sc not runnable: {err}")),
    }
}

#[cfg(not(windows))]
fn service_state(name: &str) -> Result<ServiceState, String> {
    match run_controlled(
        &["systemctl".into(), "is-active".into(), name.into()],
        Duration::from_secs(10),
        4 * 1024,
    ) {
        // is-active exits non-zero for inactive/missing units — the text is
        // still the state.
        ExecResult::Succeeded { stdout, .. } | ExecResult::Failed { stdout, .. } => Ok(match stdout.trim() {
            "active" => ServiceState::Running,
            "activating" | "reloading" => ServiceState::Pending,
            "not-found" => ServiceState::Missing,
            _ => ServiceState::Stopped,
        }),
        ExecResult::TimedOut => Err(format!("systemctl is-active {name} timed out")),
        ExecResult::StartFailed(err) => Err(format!("systemctl not runnable: {err}")),
    }
}

#[cfg(windows)]
fn client_processes() -> Result<Vec<ProcessEntry>, String> {
    let (own_pid, own_name) = own_process_identity();
    match run_controlled(
        &["tasklist".into(), "/FO".into(), "CSV".into(), "/NH".into()],
        Duration::from_secs(15),
        256 * 1024,
    ) {
        ExecResult::Succeeded { stdout, .. } => Ok(stdout
            .lines()
            .filter_map(parse_tasklist_line)
            .filter(|p| is_client_process(p, own_pid, own_name.as_deref()))
            .collect()),
        ExecResult::TimedOut => Err("tasklist timed out".into()),
        ExecResult::Failed { code, stderr, .. } => Err(format!("tasklist exit {code}: {}", stderr.trim())),
        ExecResult::StartFailed(err) => Err(format!("tasklist not runnable: {err}")),
    }
}

#[cfg(not(windows))]
fn client_processes() -> Result<Vec<ProcessEntry>, String> {
    let (own_pid, own_name) = own_process_identity();
    match run_controlled(&["ps".into(), "-eo".into(), "pid=,comm=".into()], Duration::from_secs(10), 256 * 1024) {
        ExecResult::Succeeded { stdout, .. } => Ok(stdout
            .lines()
            .filter_map(parse_ps_line)
            .filter(|p| is_client_process(p, own_pid, own_name.as_deref()))
            .collect()),
        ExecResult::TimedOut => Err("ps timed out".into()),
        ExecResult::Failed { code, stderr, .. } => Err(format!("ps exit {code}: {}", stderr.trim())),
        ExecResult::StartFailed(err) => Err(format!("ps not runnable: {err}")),
    }
}

#[cfg(windows)]
fn cloud_session(pids: &HashSet<u32>) -> Result<bool, String> {
    match run_controlled(&["netstat".into(), "-ano".into()], Duration::from_secs(15), 512 * 1024) {
        ExecResult::Succeeded { stdout, .. } => Ok(stdout.lines().any(|l| netstat_cloud_session(l, pids))),
        ExecResult::TimedOut => Err("netstat timed out".into()),
        ExecResult::Failed { code, stderr, .. } => Err(format!("netstat exit {code}: {}", stderr.trim())),
        ExecResult::StartFailed(err) => Err(format!("netstat not runnable: {err}")),
    }
}

#[cfg(not(windows))]
fn cloud_session(_: &HashSet<u32>) -> Result<bool, String> {
    match run_controlled(&["ss".into(), "-tnp".into()], Duration::from_secs(10), 512 * 1024) {
        ExecResult::Succeeded { stdout, .. } => Ok(stdout.lines().any(ss_cloud_session)),
        ExecResult::TimedOut => Err("ss timed out".into()),
        ExecResult::Failed { code, stderr, .. } => Err(format!("ss exit {code}: {}", stderr.trim())),
        ExecResult::StartFailed(err) => Err(format!("ss not runnable: {err}")),
    }
}

/// Default config.ini candidates (first readable wins).
fn config_candidates() -> Vec<std::path::PathBuf> {
    if let Some(path) = env("SUNLOGIN_CONFIG") {
        return vec![std::path::PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    if cfg!(windows) {
        candidates.push(std::path::PathBuf::from(r"C:\Program Files\Oray\AweSun\config.ini"));
        candidates.push(std::path::PathBuf::from(r"C:\ProgramData\Oray\SunloginClient\config.ini"));
    } else {
        candidates.push(std::path::PathBuf::from("/usr/local/sunlogin/config.ini"));
    }
    candidates
}

fn read_client_config() -> Option<ClientConfig> {
    config_candidates().iter().find_map(|path| {
        let text = std::fs::read_to_string(path).ok()?;
        let info = parse_client_config(&text);
        (!info.version.is_empty() || !info.account.is_empty()).then_some(info)
    })
}

/// Legacy enrichment: loopback HTTP `login-type` (11.x-era installs). Only
/// used when explicitly pointed at via env; failures are non-fatal.
fn legacy_login_type() -> Option<ClientStatus> {
    let port = if let Some(base) = env("SUNLOGIN_API") {
        base.rsplit_once(':')?.1.parse::<u16>().ok()?
    } else {
        env("SUNLOGIN_PORT")?.parse::<u16>().ok()?
    };
    let url = format!("http://127.0.0.1:{port}/cgi-bin/rpc?action=login-type");
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .build(),
    );
    let response = agent.get(&url).call().ok()?;
    let text = response.into_body().read_to_string().ok()?;
    parse_login_type(&serde_json::from_str(&text).ok()?).into()
}

/// Probe: the layered health model. `healthy` = service running AND client
/// processes alive AND cloud session established; the observation names
/// every fact (and its absence) so plans and operators see the failing
/// layer, never just a bare false.
fn probe() -> (bool, String) {
    let mut facts: Vec<String> = Vec::new();
    let mut healthy = true;

    // 1. Service layer.
    let mut service_ok = false;
    let mut service_facts = Vec::new();
    for name in service_candidates() {
        match service_state(&name) {
            Ok(ServiceState::Running) => {
                service_ok = true;
                service_facts = vec![format!("service {name} RUNNING")];
                break;
            }
            Ok(ServiceState::Pending) => service_facts.push(format!("service {name} START/STOP_PENDING")),
            Ok(ServiceState::Stopped) => service_facts.push(format!("service {name} not running (STOPPED)")),
            Ok(ServiceState::Missing) => service_facts.push(format!("service {name} not installed")),
            Err(err) => service_facts.push(err),
        }
    }
    if !service_ok {
        healthy = false;
    }
    facts.extend(service_facts);

    // 2. Client processes.
    let processes = match client_processes() {
        Ok(list) => list,
        Err(err) => {
            healthy = false;
            facts.push(err);
            Vec::new()
        }
    };
    if processes.is_empty() {
        healthy = false;
        facts.push("no client process (AweSun/Sunlogin) running".into());
    } else {
        facts.push(format!(
            "{} client process(es): {}",
            processes.len(),
            processes
                .iter()
                .map(|p| format!("{}({})", p.name, p.pid))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // 3. Cloud session (the remotely-controllable signal).
    let pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
    match cloud_session(&pids) {
        Ok(true) => facts.push("cloud session ESTABLISHED (controllable from outside)".into()),
        Ok(false) => {
            healthy = false;
            facts.push("no cloud session — not remotely controllable".into());
        }
        Err(err) => {
            healthy = false;
            facts.push(err);
        }
    }

    // 4. config.ini facts (best effort, never fatal).
    if let Some(info) = read_client_config() {
        if !info.version.is_empty() {
            facts.push(format!("version {}", info.version));
        }
        if !info.account.is_empty() {
            facts.push(format!("bound to account {}", info.account));
        }
    }

    // 5. Legacy HTTP enrichment (optional, non-fatal).
    if let Some(status) = legacy_login_type() {
        facts.push(format!(
            "legacy login-type: version {}, bound {}",
            if status.version.is_empty() { "?" } else { &status.version },
            status.bound
        ));
    }

    (healthy, facts.join("; "))
}

/// `sc start` returns the moment the SCM accepts the request — the service
/// may still be START_PENDING for seconds. The iMac live run burned both
/// plan attempts on verifications that raced the start and ended Escalated
/// while the backend was actually recovering. Poll until RUNNING so the
/// action outcome tells the truth; the plan's own timeout bounds this.
#[cfg(windows)]
fn wait_for_running(name: &str, budget: Duration) -> Result<Duration, String> {
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() >= budget {
            let last = service_state(name).unwrap_or(ServiceState::Missing);
            return Err(format!(
                "service {name} did not reach RUNNING within {}s (last state: {last:?})",
                budget.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
        if service_state(name)? == ServiceState::Running {
            return Ok(started.elapsed());
        }
    }
}

/// Restart through the platform service manager.
#[cfg(windows)]
fn restart_service() -> Result<String, String> {
    // AweSunService first; SunloginService is the legacy/broken leftover on
    // mixed installs. SUNLOGIN_SERVICE pins exactly one.
    let names = service_candidates();
    let mut last = String::new();
    for name in names {
        // `sc stop` may fail when already stopped — starting is what counts.
        let _ = run_controlled(&["sc".into(), "stop".into(), name.clone()], Duration::from_secs(60), 8 * 1024);
        match run_controlled(&["sc".into(), "start".into(), name.clone()], Duration::from_secs(60), 8 * 1024) {
            ExecResult::Succeeded { .. } => match wait_for_running(&name, Duration::from_secs(45)) {
                Ok(elapsed) => {
                    return Ok(format!("sc start {name} done; RUNNING after {}s", elapsed.as_secs().max(1)))
                }
                Err(err) => return Err(err),
            },
            ExecResult::Failed { code, stderr, .. } => {
                last = format!("`sc start {name}` exit {code}: {}", stderr.trim())
            }
            ExecResult::TimedOut => return Err(format!("sc start {name} timed out")),
            ExecResult::StartFailed(err) => last = format!("sc not runnable: {err}"),
        }
    }
    Err(last)
}

#[cfg(not(windows))]
fn restart_service() -> Result<String, String> {
    let mut last = String::new();
    for name in service_candidates() {
        match run_controlled(
            &["systemctl".to_owned(), "restart".to_owned(), name.clone()],
            Duration::from_secs(60),
            16 * 1024,
        ) {
            ExecResult::Succeeded { .. } => return Ok(format!("systemctl restart {name} done")),
            ExecResult::Failed { code, stderr, .. } => {
                last = format!("`systemctl restart {name}` exit {code}: {}", stderr.trim())
            }
            ExecResult::TimedOut => return Err("restart timed out".into()),
            ExecResult::StartFailed(err) => last = format!("systemctl not runnable: {err}"),
        }
    }
    Err(last)
}

/// Account re-login via UI automation: declared so the capability list is
/// complete, but not implemented — Windows-only, fragile (dialogs, updates
/// break selectors) and must run behind `allow_high_risk`
/// (docs/guide/plugins.md §5.8).
fn relogin() -> Result<String, String> {
    Err("sunlogin.relogin is not implemented yet: Windows UI automation, \
         pending real-machine verification"
        .into())
}

fn capabilities() -> Value {
    json!([
        {
            "kind": "probe_provider",
            "operation_id": "sunlogin.probe",
            "summary": "service + client processes + cloud session + config facts (read-only; the remotely-controllable signal)",
            "read_only": true,
            "required_permission": "probe.read",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["service_not_running", "client_not_running", "no_cloud_session"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "sunlogin.restart",
            "summary": "restart the sunlogin/awesun service (kills active remote sessions)",
            "read_only": false,
            "required_permission": "service.manage",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": true,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["service_missing", "restart_timeout"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "sunlogin.relogin",
            "summary": "re-login the Oray account via UI automation (Windows; high risk, pending implementation)",
            "read_only": false,
            "required_permission": "service.manage",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": true,
            "cancelable": false,
            "idempotent": false,
            "verifiable": true,
            "possible_failures": ["not_implemented", "automation_failed"]
        }
    ])
}

fn main() {
    serve(|method, params| match method {
        "custos/v1/initialize" => Reply::Result(json!({
            "protocol_version": "1",
            "plugin": "custos-sunlogin-plugin"
        })),
        "custos/v1/describe_capabilities" => Reply::Result(capabilities()),
        "custos/v1/list_recovery_actions" => {
            Reply::Result(json!(["sunlogin.restart", "sunlogin.relogin"]))
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
            let outcome = match operation {
                "sunlogin.restart" => restart_service(),
                "sunlogin.relogin" => relogin(),
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

    // Fixtures below are REAL device output (iMac / AweSun 16.5.0.30905,
    // captured 2026-11-16 over SSH), trimmed to the relevant lines.

    #[test]
    fn sc_query_real_fixture_states() {
        let running = "\nSERVICE_NAME: AweSunService \n        TYPE               : 110  WIN32_OWN_PROCESS  (interactive)\n        STATE              : 4  RUNNING \n                                (STOPPABLE, NOT_PAUSABLE, ACCEPTS_SHUTDOWN)\n";
        assert_eq!(parse_sc_query(running), ServiceState::Running);
        let stopped = "\nSERVICE_NAME: SunloginService \n        STATE              : 1  STOPPED \n";
        assert_eq!(parse_sc_query(stopped), ServiceState::Stopped);
        // Transition states observed live on the iMac right after `sc stop`.
        let pending = "\nSERVICE_NAME: AweSunService \n        STATE              : 3  STOP_PENDING \n                                (STOPPABLE, NOT_PAUSABLE, ACCEPTS_SHUTDOWN)\n";
        assert_eq!(parse_sc_query(pending), ServiceState::Pending);
        let starting = "\nSERVICE_NAME: AweSunService \n        STATE              : 2  START_PENDING \n";
        assert_eq!(parse_sc_query(starting), ServiceState::Pending);
        let missing = "\n[SC] EnumQueryServicesStatus:1060\n\nThe specified service does not exist as an installed service.\n";
        assert_eq!(parse_sc_query(missing), ServiceState::Missing);
    }

    #[test]
    fn tasklist_real_fixture_rows() {
        let row = "\"AweSun.exe\",\"5124\",\"Services\",\"0\",\"229,940 K\"";
        let entry = parse_tasklist_line(row).expect("parses");
        assert_eq!(entry.name, "awesun.exe");
        assert_eq!(entry.pid, 5124);
        assert!(entry.is_client());
        let guard = parse_tasklist_line("\"awesun_guard.exe\",\"10104\",\"Services\",\"0\",\"157,624 K\"")
            .expect("parses");
        assert!(guard.is_client());
        // Non-client rows are filtered by is_client, not by the parser.
        let other = parse_tasklist_line("\"explorer.exe\",\"4321\",\"Console\",\"1\",\"90,000 K\"")
            .expect("parses");
        assert!(!other.is_client());
        assert_eq!(parse_tasklist_line("garbage"), None);
    }

    #[test]
    fn self_and_sibling_plugin_processes_are_not_clients() {
        let real = ProcessEntry { name: "awesun.exe".into(), pid: 5124 };
        let own = ProcessEntry { name: "sunlogin-plugin.exe".into(), pid: 1000 };
        let sibling = ProcessEntry { name: "sunlogin-plugin.exe".into(), pid: 2000 };
        assert!(is_client_process(&real, 1000, Some("sunlogin-plugin.exe")));
        assert!(!is_client_process(&own, 1000, Some("sunlogin-plugin.exe")));
        assert!(!is_client_process(&sibling, 1000, Some("sunlogin-plugin.exe")));
    }

    #[test]
    fn netstat_real_fixture_cloud_session() {
        let mut pids = HashSet::new();
        pids.insert(5124);
        pids.insert(26568);
        let live = "  TCP    192.168.5.117:57195    47.98.147.22:443      ESTABLISHED     5124";
        assert!(netstat_cloud_session(live, &pids));
        let gui = "  TCP    192.168.5.117:49838    121.43.61.109:443      ESTABLISHED     26568";
        assert!(netstat_cloud_session(gui, &pids));
        // Foreign process / listener / non-443 are not the cloud session.
        let foreign = "  TCP    10.0.0.2:1000    1.2.3.4:443      ESTABLISHED     9999";
        assert!(!netstat_cloud_session(foreign, &pids));
        let listener = "  TCP    0.0.0.0:443    0.0.0.0:0      LISTENING       5124";
        assert!(!netstat_cloud_session(listener, &pids));
        let plain = "  TCP    192.168.5.117:5000    1.2.3.4:80      ESTABLISHED     5124";
        assert!(!netstat_cloud_session(plain, &pids));
    }

    #[test]
    fn ss_fixture_cloud_session() {
        let live = "ESTAB 0 0 192.168.5.117:57195 47.98.147.22:443 users:((\"sunloginclient\",pid=5124,fd=42))";
        assert!(ss_cloud_session(live));
        let other = "ESTAB 0 0 192.168.5.117:22 192.168.5.2:50000 users:((\"sshd\",pid=99,fd=6))";
        assert!(!ss_cloud_session(other));
        assert!(!ss_cloud_session("nothing here"));
    }

    #[test]
    fn client_config_real_fixture() {
        // Real keys from C:\Program Files\Oray\AweSun\config.ini (values
        // trimmed; encrypted blobs are simply not read).
        let ini = "[base]\nconfig_path=C:\\Program Files\\Oray\\AweSun\\config.ini\n\
                   account=orkj5118796wjwsm\nautologin=1\nfull_version=16.5.0.30905\n";
        let info = parse_client_config(ini);
        assert_eq!(info.version, "16.5.0.30905");
        assert_eq!(info.account, "orkj5118796wjwsm");
        assert_eq!(parse_client_config("[base]\nfoo=1"), ClientConfig::default());
    }

    #[test]
    fn ps_fixture_rows() {
        let entry = parse_ps_line(" 5124 sunloginclient").expect("parses");
        assert_eq!(entry.pid, 5124);
        assert!(entry.is_client());
        assert!(!parse_ps_line("99 bash").unwrap().is_client());
        assert_eq!(parse_ps_line("not-a-pid"), None);
    }

    #[test]
    fn legacy_login_type_fixture_parses() {
        let value: Value = serde_json::json!({
            "__code": 0, "code": 0,
            "version": "11.0.1.33826",
            "isbinding": "1",
            "platform": "Windows",
            "mac": "2C:F0:5D:34:23:B0"
        });
        let status = parse_login_type(&value);
        assert_eq!(status.version, "11.0.1.33826");
        assert!(status.bound);
        assert_eq!(parse_login_type(&serde_json::json!({})), ClientStatus::default());
    }

    #[test]
    fn finds_the_port_in_legacy_log_lines() {
        assert_eq!(
            find_port_in_log("[info] sunlogin_service: http listening on port 49181"),
            Some(49181)
        );
        assert_eq!(find_port_in_log("listening on port 4000"), None);
        assert_eq!(find_port_in_log(""), None);
    }

    #[test]
    fn probe_reports_the_failing_service_layer_on_any_host() {
        // Host-agnostic: pin the service to a missing unit, so the service
        // layer fails everywhere (clean dev box OR a machine with a real
        // install — process/cloud layers may legitimately succeed there).
        std::env::set_var("SUNLOGIN_SERVICE", "custos-missing-unit");
        std::env::remove_var("SUNLOGIN_API");
        std::env::remove_var("SUNLOGIN_PORT");
        std::env::remove_var("SUNLOGIN_CONFIG");

        let (healthy, observation) = probe();
        assert!(!healthy);
        assert!(observation.contains("custos-missing-unit"), "got: {observation}");
        std::env::remove_var("SUNLOGIN_SERVICE");
    }

    #[test]
    fn restart_reports_a_missing_service_cleanly() {
        std::env::set_var("SUNLOGIN_SERVICE", "custos-missing-unit");
        let err = restart_service().expect_err("no such service on this host");
        assert!(err.contains("custos-missing-unit"), "got: {err}");
        std::env::remove_var("SUNLOGIN_SERVICE");
    }

    #[test]
    fn relogin_is_declared_but_not_implemented() {
        let err = relogin().expect_err("skeleton");
        assert!(err.contains("not implemented"), "got: {err}");
    }

    #[test]
    fn capabilities_declare_the_full_operation_contract() {
        let caps = capabilities();
        let entries = caps.as_array().expect("capabilities are a list");
        assert_eq!(entries.len(), 3, "probe + two recovery actions");
        let ids: Vec<&str> = entries
            .iter()
            .filter_map(|e| e.get("operation_id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids,
            ["sunlogin.probe", "sunlogin.restart", "sunlogin.relogin"]
        );
        let probe = &entries[0];
        assert_eq!(probe["read_only"], true);
        let restart = &entries[1];
        assert_eq!(restart["interrupts_remote_admin"], true);
    }
}

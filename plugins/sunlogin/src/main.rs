//! Sunlogin (向日葵) plugin — one plugin process, one resource loop.
//!
//! 向日葵 is closed-source with no official API. The de-facto API is the
//! loopback HTTP surface the client's own UI talks to: the client listens on
//! a random high port (40000–65535, 127.0.0.1) and serves status endpoints
//! (`/cgi-bin/rpc?action=login-type` → version/binding/platform). The port
//! is written into the client's own logs, so discovery is deterministic:
//! scan the logs for the port, then query the status endpoint.
//!
//! SECURITY CONTRACT (docs/guide/plugins.md §5.8): this plugin only calls
//! read-only status endpoints, only over loopback. It never touches
//! `/check` or any historical command endpoint (the CNVD-2022-10270 RCE
//! surface, fixed upstream) — and probing `login-type` doubles as a version
//! audit for exactly that class of stale installs.
//!
//! Configuration via environment (§4.4 clean env):
//!   SUNLOGIN_API       direct status base, e.g. http://127.0.0.1:49181
//!                      (skips discovery; mostly for tests)
//!   SUNLOGIN_PORT      port override (skips log discovery)
//!   SUNLOGIN_LOG_DIR   client log directory override
//!   SUNLOGIN_SERVICE   Linux systemd unit (default sunloginclient;
//!                      oray-sunlogin tried as fallback)

use custos_plugin_api::server::{serve, Reply};
use custos_probe::exec::{run_controlled, ExecResult};
use serde_json::{json, Value};
use std::time::Duration;

/// Plausible local listening ports (client picks a random high port).
const PORT_MIN: u16 = 40_000;
const PORT_MAX: u16 = 65_535;

/// Default log directories per platform (docs: ProgramData on Windows,
/// /usr/local/sunlogin + /var/log on Linux).
#[cfg(windows)]
const DEFAULT_LOG_DIRS: [&str; 1] = [r"C:\ProgramData\Oray\SunloginClient\log"];
#[cfg(not(windows))]
const DEFAULT_LOG_DIRS: [&str; 2] = ["/usr/local/sunlogin/log", "/var/log"];

// ---------------------------------------------------------------------------
// Pure parsing helpers (unit-tested)
// ---------------------------------------------------------------------------

/// One parsed `login-type` answer — the status facts the probe reports.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientStatus {
    pub version: String,
    /// `isbinding` from the client: "1" when bound to an Oray account.
    pub bound: bool,
    pub platform: String,
    pub mac: String,
}

/// Lenient parse of `/cgi-bin/rpc?action=login-type`. Fields arrive as
/// strings ("isbinding":"1"); absence degrades to empty/unknown rather than
/// failing — the observation still carries what was learned.
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

/// Scan a log text for the client's local listening port. Best-effort across
/// known startup-line shapes (marker lines mentioning listen/port/http), to
/// be confirmed against real logs on each platform.
pub fn find_port_in_log(text: &str) -> Option<u16> {
    for line in text.lines() {
        let lower = line.to_lowercase();
        if !(lower.contains("listen") || lower.contains("port") || lower.contains("http")) {
            continue;
        }
        // Digit runs in the line; the first one in the plausible range wins.
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

/// Extract the client's listening port from one `ss -tlnp` line:
/// `LISTEN 0 128 0.0.0.0:49181 0.0.0.0:* users:(("sunloginclient",pid=..))`.
/// Any in-range port on a line naming the client process wins: the client
/// holds exactly one local listener, so no column addressing is needed.
pub fn parse_ss_line(line: &str) -> Option<u16> {
    if !line.contains("sunlogin") {
        return None;
    }
    for token in line.split_whitespace() {
        if let Some((_, port)) = token.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                if (PORT_MIN..=PORT_MAX).contains(&port) {
                    return Some(port);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Discovery and status (I/O layer)
// ---------------------------------------------------------------------------

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// Newest `*sunlogin*.log` under `dir`, by modification time.
fn newest_sunlogin_log(dir: &str) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.contains("sunlogin") && name.ends_with(".log")
        })
        .filter_map(|e| e.metadata().ok().map(|m| (e.path(), m.modified().ok())))
        .max_by_key(|(_, modified)| *modified)
        .map(|(path, _)| path)
}

/// Deterministic port discovery: explicit env overrides first, then the
/// client's own logs, then a socket-table scan for the client process.
fn discover_port() -> Result<u16, String> {
    if let Some(base) = env("SUNLOGIN_API") {
        let port = base
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .ok_or_else(|| format!("SUNLOGIN_API `{base}` carries no port"))?;
        return Ok(port);
    }
    if let Some(port) = env("SUNLOGIN_PORT").and_then(|p| p.parse::<u16>().ok()) {
        return Ok(port);
    }
    let mut dirs: Vec<String> = Vec::new();
    if let Some(dir) = env("SUNLOGIN_LOG_DIR") {
        dirs.push(dir);
    }
    dirs.extend(DEFAULT_LOG_DIRS.iter().map(|d| (*d).to_owned()));
    for dir in &dirs {
        let Some(path) = newest_sunlogin_log(dir) else {
            continue;
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(port) = find_port_in_log(&text) {
            return Ok(port);
        }
    }
    // Socket table: the listener proves the client is alive even when the
    // log moved or the version changed its line format.
    if let ExecResult::Succeeded { stdout, .. } =
        run_controlled(&["ss".into(), "-tlnp".into()], Duration::from_secs(5), 64 * 1024)
    {
        for line in stdout.lines() {
            if let Some(port) = parse_ss_line(line) {
                return Ok(port);
            }
        }
    }
    Err("client status port not discovered (client not installed, logs moved, \
         or a SUNLOGIN_PORT/SUNLOGIN_API override is needed)"
        .into())
}

/// Read-only status query over loopback — the same endpoint the client UI
/// uses. No other surface is ever contacted.
fn login_type(port: u16) -> Result<ClientStatus, String> {
    let url = format!("http://127.0.0.1:{port}/cgi-bin/rpc?action=login-type");
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .build(),
    );
    let response = agent
        .get(&url)
        .call()
        .map_err(|e| format!("client status unreachable: {e}"))?;
    let status = response.status().as_u16();
    let mut body = response.into_body();
    let text = body
        .read_to_string()
        .map_err(|e| format!("client status unreadable: {e}"))?;
    if !(200..400).contains(&status) {
        return Err(format!("client status rejected: HTTP {status}"));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("client status JSON: {e}"))?;
    Ok(parse_login_type(&value))
}

/// Probe: discovery + status in one ladder; the observation names the
/// failing stage so the plan ladder (restart vs operator work) is informed.
fn probe() -> (bool, String) {
    let port = match discover_port() {
        Ok(port) => port,
        Err(err) => return (false, err),
    };
    match login_type(port) {
        Ok(status) => {
            let binding = if status.bound {
                "bound"
            } else {
                "NOT bound to an Oray account"
            };
            let version = if status.version.is_empty() {
                "unknown version".into()
            } else {
                format!("version {}", status.version)
            };
            (
                true,
                format!("sunlogin {version}, {binding}, status port {port}"),
            )
        }
        Err(err) => (false, format!("status port {port}: {err}")),
    }
}

/// Restart through the platform service manager. Linux units first, then
/// the Windows service-control path (argv strings compile everywhere; the
/// Windows branch is pending real-machine verification, docs/guide/plugins.md §5.8).
fn restart_service() -> Result<String, String> {
    let mut units: Vec<String> = vec![env("SUNLOGIN_SERVICE").unwrap_or_else(|| "sunloginclient".into())];
    let fallback = "oray-sunlogin".to_owned();
    if !units.contains(&fallback) {
        units.push(fallback);
    }
    let mut last = String::new();
    for unit in units {
        match run_controlled(
            &["systemctl".to_owned(), "restart".to_owned(), unit.clone()],
            Duration::from_secs(60),
            16 * 1024,
        ) {
            ExecResult::Succeeded { .. } => return Ok(format!("systemctl restart {unit} done")),
            ExecResult::Failed { code, stderr, .. } => {
                last = format!("`systemctl restart {unit}` exit {code}: {}", stderr.trim());
                // Unit likely absent — fall through to the next known name.
            }
            ExecResult::TimedOut => return Err("restart timed out".into()),
            ExecResult::StartFailed(err) => last = format!("systemctl not runnable: {err}"),
        }
    }
    Err(last)
}

/// Account re-login via UI automation: declared so the capability list is
/// complete, but not implemented in this skeleton — it is Windows-only,
/// fragile (dialogs, updates break selectors) and must run behind
/// `allow_high_risk`. Pending real-machine work (docs/guide/plugins.md §5.8).
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
            "summary": "client liveness, account binding and version via the loopback status endpoint (read-only)",
            "read_only": true,
            "required_permission": "probe.read",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["client_not_discovered", "status_unreachable"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "sunlogin.restart",
            "summary": "restart the sunlogin client service (kills active remote sessions)",
            "read_only": false,
            "required_permission": "service.manage",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": true,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["unit_missing", "restart_timeout"]
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

    #[test]
    fn parse_login_type_reads_the_real_shape() {
        // Fixture from the documented login-type answer shape: string flags.
        let value: Value = serde_json::json!({
            "__code": 0, "code": 0,
            "version": "16.1.2",
            "isbinding": "1",
            "isinstalled": "1",
            "platform": "Windows",
            "mac": "aa:bb:cc:dd:ee:ff"
        });
        let status = parse_login_type(&value);
        assert_eq!(status.version, "16.1.2");
        assert!(status.bound);
        assert_eq!(status.platform, "Windows");
    }

    #[test]
    fn parse_login_type_degrades_leniently() {
        let status = parse_login_type(&serde_json::json!({}));
        assert_eq!(status, ClientStatus::default());
        assert!(!status.bound);
        // isbinding arrives as a JSON string, not a bool — "0" is unbound.
        let unbound = parse_login_type(&serde_json::json!({ "isbinding": "0" }));
        assert!(!unbound.bound);
    }

    #[test]
    fn finds_the_port_in_client_log_lines() {
        // Linux-ish startup line.
        assert_eq!(
            find_port_in_log("[info] sunlogin_service: http listening on port 49181"),
            Some(49181)
        );
        // Windows-ish line.
        assert_eq!(
            find_port_in_log("SunloginClient: local server Listen port: 52345"),
            Some(52345)
        );
        // Low ports (other services) are never mistaken for the client.
        assert_eq!(find_port_in_log("listening on port 4000"), None);
        assert_eq!(find_port_in_log("nothing relevant here"), None);
        assert_eq!(find_port_in_log(""), None);
    }

    #[test]
    fn parses_the_socket_table_line() {
        let line = "LISTEN 0 128 0.0.0.0:49181 0.0.0.0:* \
                    users:((\"sunloginclient\",pid=1234,fd=6))";
        assert_eq!(parse_ss_line(line), Some(49181));
        // Another process's listener is not the client.
        let other = "LISTEN 0 128 0.0.0.0:52345 0.0.0.0:* \
                     users:((\"nginx\",pid=99,fd=6))";
        assert_eq!(parse_ss_line(other), None);
        assert_eq!(parse_ss_line("LISTEN 0 128 0.0.0.0:22"), None);
    }

    #[test]
    fn probe_reports_the_failing_stage_on_a_clean_host() {
        // No sunlogin installed here: point discovery at an empty directory.
        let dir = std::env::temp_dir().join(format!("custos-sunlogin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::env::set_var("SUNLOGIN_LOG_DIR", &dir);
        std::env::remove_var("SUNLOGIN_PORT");
        std::env::remove_var("SUNLOGIN_API");

        let (healthy, observation) = probe();
        assert!(!healthy);
        assert!(
            observation.contains("not discovered"),
            "got: {observation}"
        );
        std::env::remove_var("SUNLOGIN_LOG_DIR");
    }

    #[test]
    fn restart_reports_each_missing_unit_cleanly() {
        std::env::set_var("SUNLOGIN_SERVICE", "custos-missing-unit");
        let err = restart_service().expect_err("no sunlogin units on this host");
        assert!(
            err.contains("custos-missing-unit") || err.contains("oray-sunlogin"),
            "got: {err}"
        );
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
        // Restarting a remote-access service kills live sessions.
        let restart = &entries[1];
        assert_eq!(restart["interrupts_remote_admin"], true);
    }
}

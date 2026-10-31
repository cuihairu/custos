//! OpenWrt adapters (docs/guide/platforms.md §11.6): procd services, UCI
//! config reads and ubus calls (todo.md P3 220).
//!
//! procd services live behind `/etc/init.d/<name>` (procd's own script
//! contract); UCI and ubus go through their CLIs under the hardened
//! runner — no shell, no blanket config overwrites. Everything refuses
//! with a concrete reason when this host is not OpenWrt.

use crate::controller::{ServiceController, ServiceState};
use crate::error::{PlatformError, PlatformResult};
use custos_probe::exec::{run_controlled, ExecResult};
use std::time::Duration;

const TOOL_TIMEOUT: Duration = Duration::from_secs(15);
const TOOL_OUTPUT_CAP: usize = 64 * 1024;

/// True when this host is OpenWrt (the canonical release marker).
pub fn is_openwrt() -> bool {
    std::path::Path::new("/etc/openwrt_release").is_file()
}

fn require_openwrt() -> PlatformResult<()> {
    if is_openwrt() {
        Ok(())
    } else {
        Err(PlatformError::Unsupported(
            "this host is not OpenWrt (/etc/openwrt_release missing)".into(),
        ))
    }
}

fn run(argv: &[String]) -> ExecResult {
    run_controlled(argv, TOOL_TIMEOUT, TOOL_OUTPUT_CAP)
}

/// procd service control via `/etc/init.d/<name>` (procd's script contract:
/// start/stop/restart/reload; status where the script implements it).
#[derive(Debug, Clone)]
pub struct ProcdServiceController;

impl ProcdServiceController {
    fn script(unit: &str) -> PlatformResult<String> {
        let path = format!("/etc/init.d/{unit}");
        if !std::path::Path::new(&path).is_file() {
            return Err(PlatformError::NotFound(format!(
                "no procd init script /etc/init.d/{unit}"
            )));
        }
        Ok(path)
    }
}

impl ServiceController for ProcdServiceController {
    fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
        require_openwrt()?;
        // `ubus call service list '{"name":"<unit>"}'` answers with the
        // procd view; a stopped service answers without instances.
        let script = Self::script(unit)?;
        let argv = vec![
            "ubus".into(),
            "call".into(),
            "service".into(),
            "list".into(),
            serde_json::json!({"name": unit}).to_string(),
        ];
        match run(&argv) {
            ExecResult::Succeeded { stdout, .. } => {
                let has_instances = serde_json::from_str::<serde_json::Value>(&stdout)
                    .map(|v| {
                        v.get(unit)
                            .is_some_and(|svc| svc.get("instances").is_some())
                    })
                    .unwrap_or(false);
                if has_instances {
                    Ok(ServiceState::Running)
                } else {
                    Ok(ServiceState::Stopped)
                }
            }
            other => {
                // ubus may be absent in minimal images: fall back to the
                // init script's own status verb when it exists.
                let _ = other;
                match run(&[script, "status".into()]) {
                    ExecResult::Succeeded { stdout, .. } => Ok(match stdout.trim() {
                        "running" => ServiceState::Running,
                        "stopped" | "inactive" => ServiceState::Stopped,
                        other => ServiceState::Unknown(other.to_owned()),
                    }),
                    ExecResult::Failed { code: 3, .. } => Ok(ServiceState::Stopped),
                    other => Err(other.into()),
                }
            }
        }
    }

    fn start(&self, unit: &str) -> PlatformResult<()> {
        require_openwrt()?;
        let script = Self::script(unit)?;
        match run(&[script, "start".into()]) {
            ExecResult::Succeeded { .. } => Ok(()),
            other => Err(other.into()),
        }
    }

    fn stop(&self, unit: &str) -> PlatformResult<()> {
        require_openwrt()?;
        let script = Self::script(unit)?;
        match run(&[script, "stop".into()]) {
            ExecResult::Succeeded { .. } => Ok(()),
            other => Err(other.into()),
        }
    }

    fn restart(&self, unit: &str) -> PlatformResult<()> {
        require_openwrt()?;
        let script = Self::script(unit)?;
        match run(&[script, "restart".into()]) {
            ExecResult::Succeeded { .. } => Ok(()),
            other => Err(other.into()),
        }
    }
}

/// Reads one UCI option (`uci get <path>`); refuses on non-OpenWrt hosts.
pub fn uci_get(path: &str) -> PlatformResult<String> {
    require_openwrt()?;
    match run(&["uci".into(), "get".into(), path.into()]) {
        ExecResult::Succeeded { stdout, .. } => Ok(stdout.trim().to_owned()),
        ExecResult::Failed { stderr, .. } => Err(PlatformError::NotFound(format!(
            "uci option {path} unset or missing ({})",
            stderr.trim()
        ))),
        other => Err(other.into()),
    }
}

/// One `ubus call <object> <method> [json]` round trip; the parsed reply.
pub fn ubus_call(
    object: &str,
    method: &str,
    params: Option<serde_json::Value>,
) -> PlatformResult<serde_json::Value> {
    require_openwrt()?;
    let mut argv = vec!["ubus".into(), "call".into(), object.into(), method.into()];
    if let Some(params) = params {
        argv.push(params.to_string());
    }
    match run(&argv) {
        ExecResult::Succeeded { stdout, .. } => serde_json::from_str(&stdout)
            .map_err(|e| PlatformError::Failed(format!("ubus reply JSON: {e}"))),
        other => Err(other.into()),
    }
}

/// Parses `ubus call network.interface.<name> status` (pure; fixture-tested):
/// the interface's `up` flag, plus route and DNS facts for observations.
pub fn parse_wan_status(status: &serde_json::Value) -> (bool, Vec<String>) {
    let up = status.get("up").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut facts = Vec::new();
    if let Some(addresses) = status.get("ipv4-address").and_then(|v| v.as_array()) {
        for address in addresses {
            if let Some(addr) = address.get("address").and_then(|v| v.as_str()) {
                facts.push(format!("address {addr}"));
            }
        }
    }
    if let Some(route) = status.get("route").and_then(|v| v.as_array()) {
        let default_routes = route
            .iter()
            .filter(|r| r.get("target").and_then(|t| t.as_str()) == Some("0.0.0.0"))
            .count();
        if default_routes > 0 {
            facts.push(format!("{default_routes} default route(s)"));
        }
    }
    (up, facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_openwrt_hosts_are_refused_not_guessed() {
        // The CI host is not OpenWrt; if it ever is, skip the refusal check.
        if is_openwrt() {
            return;
        }
        let err = ProcdServiceController.status("firewall").unwrap_err();
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
        let err = uci_get("network.lan.proto").unwrap_err();
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
        let err = ubus_call("network.interface.wan", "status", None).unwrap_err();
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
    }

    #[test]
    fn wan_status_parser_reads_up_and_facts() {
        let status = serde_json::json!({
            "up": true,
            "pending": false,
            "ipv4-address": [ { "address": "192.168.8.42", "mask": 24 } ],
            "route": [
                { "target": "0.0.0.0", "nexthop": "192.168.8.1" },
                { "target": "192.168.8.0", "nexthop": "0.0.0.0" }
            ]
        });
        let (up, facts) = parse_wan_status(&status);
        assert!(up);
        assert!(
            facts.contains(&"address 192.168.8.42".to_owned()),
            "{facts:?}"
        );
        assert!(
            facts.contains(&"1 default route(s)".to_owned()),
            "{facts:?}"
        );

        let (down, _) = parse_wan_status(&serde_json::json!({ "up": false }));
        assert!(!down);
    }
}

//! systemd unit state probe (survey §26.11, implementation order #7) —
//! the native Linux implementation of the `service` resource kind.
//!
//! Queries `systemctl show <unit> --property=ActiveState --property=SubState
//! --property=LoadState` via argv (never a shell) and normalizes: active →
//! Healthy, activating/reloading → Degraded (still coming up), failed /
//! inactive / not-found → Unhealthy (the service should be running but
//! isn't). systemd itself is detected via `/run/systemd/system`; a system
//! without it is Unreachable, and non-Linux targets reject the probe type
//! at build time (factory).

use crate::config::u64_or;
use crate::exec::{run_controlled, ExecResult};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 3_000;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Probes one systemd unit's active state.
#[derive(Debug, Clone)]
pub struct SystemdProbe {
    pub unit: String,
    pub timeout: Duration,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.11).
pub fn from_config(config: &serde_json::Value) -> Result<SystemdProbe, String> {
    let unit = required_unit(config).ok_or("`probe.unit` is required for systemd probes")?;
    Ok(SystemdProbe {
        unit,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
    })
}

fn required_unit(config: &serde_json::Value) -> Option<String> {
    let unit = config["unit"].as_str()?;
    // Any whitespace would split the argv; a unit name never contains it.
    if unit.trim().is_empty() || unit.chars().any(char::is_whitespace) {
        return None;
    }
    Some(unit.to_owned())
}

impl Probe for SystemdProbe {
    fn name(&self) -> &'static str {
        "systemd"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };

        // systemd detection: the canonical marker directory.
        if !std::path::Path::new("/run/systemd/system").exists() {
            return report(
                false,
                HealthStatus::Unreachable,
                "systemd not detected on this system".to_owned(),
            );
        }

        let argv = vec![
            "systemctl".to_owned(),
            "show".to_owned(),
            self.unit.clone(),
            "--property=ActiveState".to_owned(),
            "--property=SubState".to_owned(),
            "--property=LoadState".to_owned(),
        ];
        let result = run_controlled(&argv, self.timeout, MAX_OUTPUT_BYTES);
        let stdout = match &result {
            ExecResult::Succeeded { stdout, .. } => stdout.clone(),
            ExecResult::TimedOut => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("systemctl show timed out for {}", self.unit),
                );
            }
            ExecResult::StartFailed(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot run systemctl: {err}"),
                );
            }
            ExecResult::Failed { code, stderr, .. } => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!(
                        "systemctl show exited {code}: {}",
                        stderr.lines().next().unwrap_or("")
                    ),
                );
            }
        };

        let properties = parse_properties(&stdout);
        let load = properties.get("LoadState").map(String::as_str);
        let active = properties
            .get("ActiveState")
            .map(String::as_str)
            .unwrap_or("");
        let sub = properties.get("SubState").map(String::as_str).unwrap_or("");

        if load == Some("not-found") {
            return report(
                false,
                HealthStatus::Unhealthy,
                format!("unit {} not found", self.unit),
            );
        }
        let (status, verdict) = match active {
            "active" => (HealthStatus::Healthy, "active"),
            "activating" | "reloading" => (HealthStatus::Degraded, active),
            "failed" | "inactive" | "deactivating" => (HealthStatus::Unhealthy, active),
            "" => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    "systemctl output has no ActiveState".to_owned(),
                );
            }
            other => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("unrecognized ActiveState {other:?}"),
                );
            }
        };
        report(
            status == HealthStatus::Healthy,
            status,
            format!("{verdict} ({sub})"),
        )
    }
}

/// Parses `Key=Value` lines from systemctl show output.
fn parse_properties(stdout: &str) -> std::collections::HashMap<String, String> {
    stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource() -> Resource {
        Resource::new("nginx", "service")
    }

    #[test]
    fn from_config_requires_unit() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"unit": "  "})).is_err());
        assert!(from_config(&json!({"unit": "a b"})).is_err());
        assert!(from_config(&json!({"unit": "nginx.service"})).is_ok());
    }

    #[test]
    fn parse_properties_reads_key_value_lines() {
        let props = parse_properties("ActiveState=active\nSubState=running\nId=nginx.service\n");
        assert_eq!(props.get("ActiveState").map(String::as_str), Some("active"));
        assert_eq!(props.get("SubState").map(String::as_str), Some("running"));
        assert_eq!(props.get("Id").map(String::as_str), Some("nginx.service"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn real_systemd_if_present_reports_well_formed() {
        // On a systemd host the unit may or may not exist; the probe must
        // return a definite status either way (not Unknown).
        let probe = SystemdProbe {
            unit: "dbus.service".into(),
            timeout: Duration::from_secs(3),
        };
        let report = probe.probe(&resource());
        assert_ne!(
            report.status,
            HealthStatus::Unknown,
            "{}",
            report.observation
        );
    }
}

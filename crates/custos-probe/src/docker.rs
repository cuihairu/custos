//! Docker container state probe (survey §26.2/§26.11, implementation
//! order #8).
//!
//! Runs `docker inspect -f '<go template>' <container>` via argv (never a
//! shell) — the canonical interface, no HTTP-over-socket plumbing. One
//! call reads State.Status and State.RestartCount: not-running →
//! Unhealthy, running but restarts above `max_restarts` → Degraded
//! (crash-looping), otherwise Healthy. A missing container is a definite
//! answer (Unhealthy); a missing docker binary or timeout is Unreachable.

use crate::config::{optional_str, u64_or};
use crate::exec::{run_controlled, ExecResult};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// Probes one Docker container by name or ID.
#[derive(Debug, Clone)]
pub struct DockerProbe {
    pub container: String,
    pub timeout: Duration,
    /// RestartCount at or above this marks a running container Degraded
    /// (crash-loop); None disables the check.
    pub max_restarts: Option<u64>,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.2).
pub fn from_config(config: &serde_json::Value) -> Result<DockerProbe, String> {
    let container = optional_str(config, "container")
        .ok_or("`probe.container` is required for docker-container")?;
    if container.trim().is_empty() || container.chars().any(char::is_whitespace) {
        return Err("`probe.container` must be a single name or ID".to_owned());
    }
    let max_restarts = {
        let value = u64_or(config, "max_restarts", u64::MAX);
        (value != u64::MAX).then_some(value)
    };
    Ok(DockerProbe {
        container,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
        max_restarts,
    })
}

impl Probe for DockerProbe {
    fn name(&self) -> &'static str {
        "docker"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };

        // One template element, passed as a single argv item — no shell.
        let argv = vec![
            "docker".to_owned(),
            "inspect".to_owned(),
            "--format".to_owned(),
            "{{.State.Status}} {{.State.Running}} {{.State.RestartCount}}".to_owned(),
            self.container.clone(),
        ];
        match run_controlled(&argv, self.timeout, 64 * 1024) {
            ExecResult::Succeeded { stdout, .. } => self.evaluate(&report, stdout.trim()),
            ExecResult::Failed { code, stderr, .. } => {
                let first = stderr.lines().next().unwrap_or("");
                if first.contains("No such object") {
                    report(
                        false,
                        HealthStatus::Unhealthy,
                        format!("container {} does not exist", self.container),
                    )
                } else {
                    report(
                        false,
                        HealthStatus::Unreachable,
                        format!("docker inspect exited {code}: {first}"),
                    )
                }
            }
            ExecResult::TimedOut => report(
                false,
                HealthStatus::Unreachable,
                "docker inspect timed out".to_owned(),
            ),
            ExecResult::StartFailed(err) => report(
                false,
                HealthStatus::Unreachable,
                format!("cannot run docker (is it installed and on PATH?): {err}"),
            ),
        }
    }
}

impl DockerProbe {
    fn evaluate(
        &self,
        report: &impl Fn(bool, HealthStatus, String) -> ProbeReport,
        stdout: &str,
    ) -> ProbeReport {
        // "running true 3" — status, running flag, restart count.
        const KNOWN_STATUSES: [&str; 8] = [
            "created",
            "restarting",
            "running",
            "removing",
            "paused",
            "exited",
            "dead",
            "unknown",
        ];
        let mut fields = stdout.split_whitespace();
        let (status, running, restarts) = match (fields.next(), fields.next(), fields.next()) {
            (Some(status), Some(running), Some(restarts)) => {
                (status, running, restarts.parse::<u64>().unwrap_or(0))
            }
            _ => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("unexpected docker inspect output: {stdout:?}"),
                );
            }
        };
        if !KNOWN_STATUSES.contains(&status) {
            return report(
                false,
                HealthStatus::Unreachable,
                format!("unexpected docker inspect output: {stdout:?}"),
            );
        }
        let Ok(running) = running.parse::<bool>() else {
            return report(
                false,
                HealthStatus::Unreachable,
                format!("unexpected docker inspect output: {stdout:?}"),
            );
        };
        if !running {
            return report(
                false,
                HealthStatus::Unhealthy,
                format!("container is {status} (not running)"),
            );
        }
        if let Some(max) = self.max_restarts {
            if restarts >= max {
                return report(
                    false,
                    HealthStatus::Degraded,
                    format!("running but restarted {restarts} times (>= {max})"),
                );
            }
        }
        report(
            true,
            HealthStatus::Healthy,
            format!("running, {restarts} restart(s)"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource() -> Resource {
        Resource::new("web", "docker-container")
    }

    fn probe(max_restarts: Option<u64>) -> DockerProbe {
        DockerProbe {
            container: "web".into(),
            timeout: Duration::from_secs(5),
            max_restarts,
        }
    }

    fn evaluate(stdout: &str, max_restarts: Option<u64>) -> ProbeReport {
        let p = probe(max_restarts);
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: ResourceId::new("web"),
            probe_name: "docker".into(),
            succeeded,
            status,
            observation,
        };
        p.evaluate(&report, stdout)
    }

    use custos_model::ids::ResourceId;

    #[test]
    fn running_container_is_healthy() {
        let report = evaluate("running true 0", None);
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.observation.contains("0 restart"));
    }

    #[test]
    fn exited_container_is_unhealthy() {
        let report = evaluate("exited false 2", None);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(report.observation.contains("not running"));
    }

    #[test]
    fn paused_container_is_unhealthy() {
        let report = evaluate("paused false 0", None);
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn high_restart_count_is_degraded() {
        let report = evaluate("running true 5", Some(3));
        assert_eq!(report.status, HealthStatus::Degraded);
        assert!(report.observation.contains("restarted 5"));
    }

    #[test]
    fn restart_count_below_threshold_is_healthy() {
        let report = evaluate("running true 2", Some(3));
        assert_eq!(report.status, HealthStatus::Healthy);
    }

    #[test]
    fn garbage_output_is_unreachable() {
        let report = evaluate("some docker warning\nthen nothing", None);
        assert_eq!(report.status, HealthStatus::Unreachable);
    }

    #[test]
    fn from_config_requires_container() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"container": "a b"})).is_err());
        let p = from_config(&json!({"container": "web", "max_restarts": 3})).unwrap();
        assert_eq!(p.max_restarts, Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn without_docker_binary_reports_unreachable() {
        // On a host without docker on PATH the probe must degrade to
        // Unreachable, never panic. (With docker present this asserts the
        // well-formed path instead.)
        let report = probe(None).probe(&resource());
        match std::process::Command::new("docker")
            .arg("--version")
            .output()
        {
            Ok(_) => assert_ne!(report.status, HealthStatus::Unknown),
            Err(_) => assert_eq!(report.status, HealthStatus::Unreachable),
        }
    }
}

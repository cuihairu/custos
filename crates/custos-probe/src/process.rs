//! Process Probe: checks that a PID from a pid file is still alive
//! (docs/guide/probe-diagnosis.md §5.4).

use custos_model::{HealthStatus, Resource};

pub struct ProcessProbe {
    pub pid_file: std::path::PathBuf,
}

impl crate::Probe for ProcessProbe {
    fn name(&self) -> &'static str {
        "process"
    }

    fn probe(&self, resource: &Resource) -> crate::ProbeReport {
        let report = |succeeded, status, observation| crate::ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let pid = match std::fs::read_to_string(&self.pid_file) {
            Ok(text) => match text.trim().parse::<u32>() {
                Ok(pid) => pid,
                Err(_) => {
                    return report(
                        false,
                        HealthStatus::Unhealthy,
                        format!("pid file {} has no valid pid", self.pid_file.display()),
                    );
                }
            },
            Err(err) => {
                // The pid file itself is missing/unreadable: the supervised
                // process is gone, not "unreachable".
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    format!("cannot read pid file {}: {err}", self.pid_file.display()),
                );
            }
        };
        if is_alive(pid) {
            report(true, HealthStatus::Healthy, format!("pid {pid} alive"))
        } else {
            report(false, HealthStatus::Unhealthy, format!("pid {pid} exited"))
        }
    }
}

fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(unix))]
    {
        // Windows: tasklist exits 0 when the pid exists.
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Probe;

    fn resource(id: &str) -> Resource {
        Resource::new(id, "process")
    }

    #[test]
    fn alive_pid_reports_healthy() {
        let dir = std::env::temp_dir().join(format!("custos-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("alive.pid");
        std::fs::write(&pid_file, std::process::id().to_string()).unwrap();

        let report = ProcessProbe { pid_file }.probe(&resource("worker"));
        assert!(report.succeeded);
        assert_eq!(report.status, HealthStatus::Healthy);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exited_pid_reports_unhealthy() {
        let dir = std::env::temp_dir().join(format!("custos-probe-dead-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A pid that is guaranteed not to exist on this system.
        std::fs::write(dir.join("dead.pid"), "4194303").unwrap();

        let report = ProcessProbe {
            pid_file: dir.join("dead.pid"),
        }
        .probe(&resource("worker"));
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_pid_file_reports_unhealthy() {
        let report = ProcessProbe {
            pid_file: std::path::PathBuf::from("/nonexistent/custos/pid"),
        }
        .probe(&resource("worker"));
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }
}

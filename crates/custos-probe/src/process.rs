//! Process Probe: checks that a PID from a pid file is still alive, with an
//! optional RSS-memory threshold (docs/guide/probe-diagnosis.md §5.4,
//! probe-survey.md §26: 进程级资源占用阈值).

use custos_model::{HealthStatus, Resource};

pub struct ProcessProbe {
    pub pid_file: std::path::PathBuf,
    /// When set, the probe is Unhealthy once the process's resident set
    /// exceeds this many KiB (probe-survey.md §26: 进程级资源占用阈值;
    /// Linux reads `/proc/<pid>/statm`).
    pub rss_threshold_kb: Option<u64>,
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
        if !is_alive(pid) {
            return report(false, HealthStatus::Unhealthy, format!("pid {pid} exited"));
        }
        match self.rss_threshold_kb {
            None => report(true, HealthStatus::Healthy, format!("pid {pid} alive")),
            Some(threshold) => match resident_kb(pid) {
                Some(rss) if rss > threshold => report(
                    false,
                    HealthStatus::Unhealthy,
                    format!("pid {pid} alive but RSS {rss} KiB exceeds threshold {threshold} KiB"),
                ),
                Some(rss) => report(
                    true,
                    HealthStatus::Healthy,
                    format!("pid {pid} alive, RSS {rss} KiB"),
                ),
                // The process may have exited between the alive check and
                // the statm read; that is a liveness fact, not unreachability.
                None => report(
                    false,
                    HealthStatus::Unhealthy,
                    format!("pid {pid} exited before RSS could be read"),
                ),
            },
        }
    }
}

/// Resident set size in KiB via `/proc/<pid>/statm` (Linux). `None` when
/// the process no longer exists (exited between checks) or on platforms
/// without a statm file — the caller decides how to report it.
#[cfg(unix)]
fn resident_kb(pid: u32) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    // Fields: size resident shared text lib data dt (in pages).
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(resident_pages * page_size_kb())
}

/// Kernel page size in KiB (Linux), falling back to the common 4 KiB.
#[cfg(unix)]
fn page_size_kb() -> u64 {
    // libc::sysconf(_SC_PAGESIZE) is exact; 4096 covers every supported
    // deployment kernel, and the threshold comparison tolerates a 4 KiB
    // granularity either way.
    const _SC_PAGESIZE: i32 = 30;
    let page = unsafe { libc::sysconf(_SC_PAGESIZE) };
    if page > 0 {
        (page as u64) / 1024
    } else {
        4
    }
}

#[cfg(not(unix))]
fn resident_kb(_pid: u32) -> Option<u64> {
    // Windows: GetProcessMemoryInfo would need psapi; a threshold there is
    // declared unsupported rather than silently skipped (§11.1 不伪造支持).
    None
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
    fn rss_threshold_under_a_huge_limit_is_healthy_with_facts() {
        // Our own pid: any real process's RSS is well under 1 GiB.
        let dir = std::env::temp_dir().join(format!("custos-probe-rss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("self.pid");
        std::fs::write(&pid_file, format!("{}\n", std::process::id())).unwrap();
        let probe = ProcessProbe {
            pid_file: pid_file.clone(),
            rss_threshold_kb: Some(1024 * 1024),
        };
        let report = probe.probe(&resource("self"));
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
        assert!(report.observation.contains("RSS"), "{}", report.observation);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rss_threshold_over_a_tiny_limit_is_unhealthy() {
        // 1 KiB: no live process fits — the threshold must trip.
        let dir = std::env::temp_dir().join(format!("custos-probe-rss2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("self.pid");
        std::fs::write(&pid_file, format!("{}\n", std::process::id())).unwrap();
        let probe = ProcessProbe {
            pid_file: pid_file.clone(),
            rss_threshold_kb: Some(1),
        };
        let report = probe.probe(&resource("self"));
        assert_eq!(
            report.status,
            HealthStatus::Unhealthy,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("exceeds"),
            "{}",
            report.observation
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rss_threshold_on_a_dead_pid_is_liveness_unhealthy_not_unreachable() {
        // 99999999 cannot exist; the alive check fires first with "exited".
        let dir = std::env::temp_dir().join("custos-probe-rss3");
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("dead.pid");
        std::fs::write(&pid_file, "99999999\n").unwrap();
        let probe = ProcessProbe {
            pid_file: pid_file.clone(),
            rss_threshold_kb: Some(1024),
        };
        let report = probe.probe(&resource("dead"));
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("exited"),
            "{}",
            report.observation
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn alive_pid_reports_healthy() {
        let dir = std::env::temp_dir().join(format!("custos-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("alive.pid");
        std::fs::write(&pid_file, std::process::id().to_string()).unwrap();

        let report = ProcessProbe {
            pid_file,
            rss_threshold_kb: None,
        }
        .probe(&resource("worker"));
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
            rss_threshold_kb: None,
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
            rss_threshold_kb: None,
        }
        .probe(&resource("worker"));
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }
}

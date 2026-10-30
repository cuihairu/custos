//! CPU probe (survey §26.5, implementation order #5).
//!
//! Two signals, reported as the worse of the two:
//! - **load average** (Linux only): 1-minute load from `/proc/loadavg`
//!   divided by the online core count, classified by a per-core threshold
//!   (a load of 1.0/core is a fully-loaded system).
//! - **CPU %**: a differential sample — Linux `/proc/stat` across two
//!   probe() calls, Windows `GetSystemTimes`. The first call has no prior
//!   sample and contributes only the load signal.
//!
//! The probe is stateful across calls (it holds the previous sample), so
//! the scheduler's probe interval is the sampling window.

use crate::sys::{load_threshold, pct_threshold, LoadThreshold, PctThreshold};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::sync::Mutex;

/// Cumulative CPU counter snapshot (kernel ticks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTimes {
    idle: u64,
    busy: u64,
}

impl CpuTimes {
    /// Busy percentage across a window, or None if the window is empty.
    fn busy_pct_between(&self, prev: &CpuTimes) -> Option<u64> {
        let idle_delta = self.idle.checked_sub(prev.idle)?;
        let busy_delta = self.busy.checked_sub(prev.busy)?;
        let total_delta = idle_delta + busy_delta;
        if total_delta == 0 {
            return None;
        }
        Some(
            (busy_delta
                .saturating_mul(100)
                .min(total_delta.saturating_mul(100)))
                / total_delta,
        )
    }
}

/// Probes CPU load and utilization.
#[derive(Debug)]
pub struct CpuProbe {
    pub load: LoadThreshold,
    pub cpu_pct: PctThreshold,
    /// Previous sample for the differential CPU% (empty on first call).
    prev: Mutex<Option<CpuTimes>>,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.5).
pub fn from_config(config: &serde_json::Value) -> Result<CpuProbe, String> {
    Ok(CpuProbe {
        load: load_threshold(config)?,
        cpu_pct: pct_threshold(config, "cpu")?,
        prev: Mutex::new(None),
    })
}

impl Probe for CpuProbe {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };

        let current = match sample() {
            Ok(sample) => sample,
            Err(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot read cpu counters: {err}"),
                );
            }
        };

        let mut status = HealthStatus::Healthy;
        let mut parts: Vec<String> = Vec::new();

        // Load average (Linux): 1min load / online cores.
        match load_per_core() {
            Ok(Some(load_per_core)) => {
                let load_status = self.load.classify(load_per_core);
                status = worse(status, load_status);
                parts.push(format!("load {load_per_core:.2}/core"));
            }
            Ok(None) => {} // no load signal on this platform
            Err(err) => parts.push(format!("load unavailable: {err}")),
        }

        // Differential CPU%.
        let mut prev = self
            .prev
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match prev.and_then(|old| current.busy_pct_between(&old)) {
            Some(pct) => {
                let cpu_status = self.cpu_pct.classify(pct);
                status = worse(status, cpu_status);
                parts.push(format!("cpu {pct}%"));
            }
            None => parts.push("cpu sampling (first interval)".to_owned()),
        }
        *prev = Some(current);

        report(status == HealthStatus::Healthy, status, parts.join("; "))
    }
}

/// Reads the current cumulative CPU counters.
#[cfg(unix)]
fn sample() -> Result<CpuTimes, String> {
    let content =
        std::fs::read_to_string("/proc/stat").map_err(|err| format!("read /proc/stat: {err}"))?;
    let line = content
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or("/proc/stat has no aggregate `cpu` line".to_owned())?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8) // user nice system idle iowait irq softirq steal
        .map(|field| field.parse().unwrap_or(0))
        .collect();
    if fields.len() < 5 {
        return Err("malformed `cpu` line in /proc/stat".to_owned());
    }
    let user = fields[0];
    let nice = fields[1];
    let system = fields[2];
    let idle = fields[3];
    let iowait = fields[4];
    let irq = fields.get(5).copied().unwrap_or(0);
    let softirq = fields.get(6).copied().unwrap_or(0);
    let steal = fields.get(7).copied().unwrap_or(0);
    // guest/guest_nice (indices 8/9) are already folded into user/nice.
    let idle_total = idle + iowait;
    let busy = user + nice + system + irq + softirq + steal;
    Ok(CpuTimes {
        idle: idle_total,
        busy,
    })
}

#[cfg(windows)]
fn sample() -> Result<CpuTimes, String> {
    use windows_sys::Win32::Foundation::FILETIME;
    type FileTime = FILETIME;
    // GetSystemTimes is not exposed by windows-sys 0.61; declare it. Note
    // Windows kernel time *includes* idle, so busy = user + (kernel - idle).
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(
            lp_idle_time: *mut FileTime,
            lp_kernel_time: *mut FileTime,
            lp_user_time: *mut FileTime,
        ) -> i32;
    }
    fn to_u64(ft: FileTime) -> u64 {
        (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
    }
    let mut idle = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return Err(format!(
            "GetSystemTimes failed (win32 error {})",
            std::io::Error::last_os_error()
        ));
    }
    let idle = to_u64(idle);
    let kernel = to_u64(kernel);
    let user = to_u64(user);
    Ok(CpuTimes {
        idle,
        busy: user.saturating_add(kernel.saturating_sub(idle)),
    })
}

#[cfg(not(any(unix, windows)))]
fn sample() -> Result<CpuTimes, String> {
    Err("cpu counters are unsupported on this platform".to_owned())
}

/// 1-minute load average per online core (Linux only).
#[cfg(unix)]
fn load_per_core() -> Result<Option<f64>, String> {
    let content = std::fs::read_to_string("/proc/loadavg")
        .map_err(|err| format!("read /proc/loadavg: {err}"))?;
    let load1: f64 = content
        .split_whitespace()
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or("malformed /proc/loadavg".to_owned())?;
    let cores = online_cores();
    if cores == 0 {
        return Ok(None);
    }
    Ok(Some(load1 / cores as f64))
}

#[cfg(not(unix))]
fn load_per_core() -> Result<Option<f64>, String> {
    Ok(None)
}

#[cfg(unix)]
fn online_cores() -> u32 {
    // `_SC_NPROCESSORS_ONLN` is portable across POSIX platforms.
    let cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if cores > 0 {
        cores as u32
    } else {
        // Fallback: parse /proc/cpuinfo processor entries.
        std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .map(|content| content.matches("processor").count() as u32)
            .unwrap_or(1)
    }
}

/// Reports the more severe of two statuses.
fn worse(a: HealthStatus, b: HealthStatus) -> HealthStatus {
    match (a, b) {
        (HealthStatus::Unhealthy, _) | (_, HealthStatus::Unhealthy) => HealthStatus::Unhealthy,
        (HealthStatus::Degraded, _) | (_, HealthStatus::Degraded) => HealthStatus::Degraded,
        _ => HealthStatus::Healthy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource() -> Resource {
        Resource::new("cpu", "cpu")
    }

    #[test]
    fn busy_pct_between_computes_window_usage() {
        let a = CpuTimes { idle: 90, busy: 10 };
        let b = CpuTimes {
            idle: 90 + 80, // 80 more idle ticks
            busy: 10 + 20, // 20 more busy ticks
        };
        // 20 busy / 100 total = 20%.
        assert_eq!(b.busy_pct_between(&a), Some(20));
    }

    #[test]
    fn busy_pct_returns_none_for_empty_or_rewound_window() {
        let a = CpuTimes {
            idle: 100,
            busy: 50,
        };
        // Equal counters: empty window.
        assert_eq!(a.busy_pct_between(&a), None);
        // Rewound (counter reset): no valid delta.
        let b = CpuTimes { idle: 1, busy: 1 };
        assert_eq!(b.busy_pct_between(&a), None);
    }

    #[test]
    fn from_config_reads_thresholds() {
        let probe = from_config(&json!({
            "warn_load": 0.5, "crit_load": 0.9,
            "warn_pct": 75, "crit_pct": 90,
        }))
        .unwrap();
        assert_eq!(probe.load.warn_per_core, 0.5);
        assert_eq!(probe.cpu_pct.warn_pct, 75);
    }

    #[test]
    fn from_config_rejects_bad_load_order() {
        assert!(from_config(&json!({"warn_load": 1.0, "crit_load": 0.5})).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn real_cpu_report_is_well_formed() {
        let probe = CpuProbe {
            load: LoadThreshold {
                warn_per_core: 0.7,
                crit_per_core: 1.0,
            },
            cpu_pct: PctThreshold {
                warn_pct: 80,
                crit_pct: 90,
            },
            prev: Mutex::new(None),
        };
        // First call: load signal present, cpu% warming up.
        let first = probe.probe(&resource());
        assert_ne!(first.status, HealthStatus::Unknown);
        assert!(first.observation.contains("load"), "{}", first.observation);
        // Second call: both signals present.
        let second = probe.probe(&resource());
        assert_ne!(second.status, HealthStatus::Unknown);
        assert!(second.observation.contains("cpu"), "{}", second.observation);
    }
}

//! Memory probe (survey §26.5, implementation order #5).
//!
//! Linux reads `/proc/meminfo` (MemAvailable — the kernel's own estimate of
//! memory available for new workloads); Windows calls `GlobalMemoryStatusEx`.
//! An optional swap check (SwapTotal/SwapFree) reuses the same threshold
//! model and reports the worse of the two. An unreadable metric is
//! Unreachable (the probe could not obtain a result).

use crate::config::bool_or;
use crate::sys::{pct_threshold, percent, PctThreshold};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};

/// Probes memory (and optionally swap) usage.
#[derive(Debug, Clone)]
pub struct MemProbe {
    pub threshold: PctThreshold,
    pub check_swap: bool,
    pub swap_threshold: PctThreshold,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.5).
pub fn from_config(config: &serde_json::Value) -> Result<MemProbe, String> {
    Ok(MemProbe {
        threshold: pct_threshold(config, "memory")?,
        check_swap: bool_or(config, "check_swap", false),
        swap_threshold: pct_threshold(config, "swap")?,
    })
}

impl Probe for MemProbe {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let metrics = match memory() {
            Ok(metrics) => metrics,
            Err(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot read memory metrics: {err}"),
                );
            }
        };
        let mem_pct = percent(metrics.used, metrics.total);
        let mut status = self.threshold.classify(mem_pct);
        let mut observation = format!(
            "{:.1}% memory used ({} of {} available)",
            mem_pct as f64,
            human(metrics.available),
            human(metrics.total)
        );

        if self.check_swap && metrics.swap_total > 0 {
            let swap_used = metrics.swap_total.saturating_sub(metrics.swap_free);
            let swap_pct = percent(swap_used, metrics.swap_total);
            let swap_status = self.swap_threshold.classify(swap_pct);
            if swap_status != HealthStatus::Healthy {
                status = worse(status, swap_status);
                observation.push_str(&format!(
                    "; {:.1}% swap used ({} of {} free)",
                    swap_pct as f64,
                    human(metrics.swap_free),
                    human(metrics.swap_total)
                ));
            }
        }
        report(status == HealthStatus::Healthy, status, observation)
    }
}

/// RAM metrics (bytes).
#[derive(Debug, Clone, Copy)]
struct MemoryMetrics {
    total: u64,
    available: u64,
    used: u64,
    swap_total: u64,
    swap_free: u64,
}

#[cfg(unix)]
fn memory() -> Result<MemoryMetrics, String> {
    use std::collections::HashMap;
    let content = std::fs::read_to_string("/proc/meminfo")
        .map_err(|err| format!("read /proc/meminfo: {err}"))?;
    let mut fields = HashMap::new();
    for line in content.lines() {
        if let Some((key, rest)) = line.split_once(':') {
            let kb: u64 = rest
                .trim()
                .trim_end_matches(" kB")
                .trim()
                .parse()
                .unwrap_or(0);
            fields.insert(key.trim().to_owned(), kb);
        }
    }
    let get = |key: &str| fields.get(key).copied().unwrap_or(0);
    let total = get("MemTotal").saturating_mul(1024);
    let available = get("MemAvailable").saturating_mul(1024);
    let swap_total = get("SwapTotal").saturating_mul(1024);
    let swap_free = get("SwapFree").saturating_mul(1024);
    if total == 0 {
        return Err("/proc/meminfo has no MemTotal".to_owned());
    }
    Ok(MemoryMetrics {
        total,
        available,
        used: total.saturating_sub(available),
        swap_total,
        swap_free,
    })
}

#[cfg(windows)]
fn memory() -> Result<MemoryMetrics, String> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        return Err(format!(
            "GlobalMemoryStatusEx failed (win32 error {})",
            std::io::Error::last_os_error()
        ));
    }
    let total = status.ullTotalPhys;
    let available = status.ullAvailPhys;
    Ok(MemoryMetrics {
        total,
        available,
        used: total.saturating_sub(available),
        swap_total: 0, // page-file semantics differ; not a simple swap pair
        swap_free: 0,
    })
}

#[cfg(not(any(unix, windows)))]
fn memory() -> Result<MemoryMetrics, String> {
    Err("memory metrics are unsupported on this platform".to_owned())
}

/// Reports the more severe of two statuses (Unhealthy > Degraded > Healthy).
fn worse(a: HealthStatus, b: HealthStatus) -> HealthStatus {
    match (a, b) {
        (HealthStatus::Unhealthy, _) | (_, HealthStatus::Unhealthy) => HealthStatus::Unhealthy,
        (HealthStatus::Degraded, _) | (_, HealthStatus::Degraded) => HealthStatus::Degraded,
        _ => HealthStatus::Healthy,
    }
}

fn human(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource() -> Resource {
        Resource::new("mem", "memory")
    }

    #[test]
    fn from_config_reads_thresholds_and_swap_flag() {
        let probe =
            from_config(&json!({"warn_pct": 70, "crit_pct": 85, "check_swap": true})).unwrap();
        assert_eq!(probe.threshold.warn_pct, 70);
        assert!(probe.check_swap);
    }

    #[test]
    fn from_config_swap_off_by_default() {
        let probe = from_config(&json!({})).unwrap();
        assert!(!probe.check_swap);
    }

    #[test]
    fn worse_ranks_severity() {
        assert_eq!(
            worse(HealthStatus::Degraded, HealthStatus::Unhealthy),
            HealthStatus::Unhealthy
        );
        assert_eq!(
            worse(HealthStatus::Unhealthy, HealthStatus::Healthy),
            HealthStatus::Unhealthy
        );
        assert_eq!(
            worse(HealthStatus::Healthy, HealthStatus::Degraded),
            HealthStatus::Degraded
        );
        assert_eq!(
            worse(HealthStatus::Healthy, HealthStatus::Healthy),
            HealthStatus::Healthy
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_memory_report_is_well_formed() {
        let probe = MemProbe {
            threshold: PctThreshold {
                warn_pct: 80,
                crit_pct: 90,
            },
            check_swap: false,
            swap_threshold: PctThreshold {
                warn_pct: 50,
                crit_pct: 70,
            },
        };
        let report = probe.probe(&resource());
        assert_ne!(report.status, HealthStatus::Unknown);
        assert!(
            report.observation.contains("memory used"),
            "{}",
            report.observation
        );
    }
}

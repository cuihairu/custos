//! Disk space probe (survey §26.5, implementation order #5).
//!
//! Reads free/total bytes of one mount point and classifies the used
//! percentage by warn/crit thresholds. Linux uses `statvfs(2)`; Windows
//! uses `GetDiskFreeSpaceExW`. A mount point that cannot be read at all is
//! Unreachable (the probe could not obtain a result).

use crate::config::required_str;
use crate::sys::{pct_threshold, percent, PctThreshold};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};

/// Probes the used-space percentage of one mount point.
#[derive(Debug, Clone)]
pub struct DiskProbe {
    pub mount_point: String,
    pub threshold: PctThreshold,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.5).
pub fn from_config(config: &serde_json::Value) -> Result<DiskProbe, String> {
    let mount_point = required_str(config, "mount_point")
        .ok_or("`probe.mount_point` is required for disk probes")?;
    if mount_point.trim().is_empty() {
        return Err("`probe.mount_point` must not be empty".to_owned());
    }
    Ok(DiskProbe {
        mount_point,
        threshold: pct_threshold(config, "disk")?,
    })
}

impl Probe for DiskProbe {
    fn name(&self) -> &'static str {
        "disk"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let (used, total, free) = match usage(&self.mount_point) {
            Ok(usage) => usage,
            Err(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot read usage of {}: {err}", self.mount_point),
                );
            }
        };
        let pct = percent(used, total);
        let status = self.threshold.classify(pct);
        report(
            status == HealthStatus::Healthy,
            status,
            format!(
                "{:.1}% used ({} of {} free)",
                pct as f64,
                human(free),
                human(total)
            ),
        )
    }
}

/// Returns (used_bytes, total_bytes, free_bytes) for a mount point.
#[cfg(unix)]
// statvfs fields are u64 on 64-bit targets and u32 on 32-bit ones; the
// widening cast is required on the latter, so allow it unconditionally.
#[allow(clippy::unnecessary_cast)]
fn usage(mount_point: &str) -> Result<(u64, u64, u64), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::OsStr::new(mount_point).as_bytes();
    let path = CString::new(path).map_err(|_| "mount point contains a NUL byte".to_owned())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(format!(
            "statvfs failed (errno {})",
            std::io::Error::last_os_error()
        ));
    }
    let stat = unsafe { stat.assume_init() };
    let frsize = if stat.f_frsize == 0 {
        stat.f_bsize as u64
    } else {
        stat.f_frsize as u64
    };
    if frsize == 0 {
        return Err("statvfs reported a zero fragment size".to_owned());
    }
    let total = (stat.f_blocks as u64).saturating_mul(frsize);
    // f_bavail = bytes available to an unprivileged process (like the
    // Windows lpFreeBytesAvailableToCaller used below).
    let free = (stat.f_bavail as u64).saturating_mul(frsize);
    Ok((total.saturating_sub(free), total, free))
}

/// Returns (used_bytes, total_bytes, free_bytes) for a volume.
#[cfg(windows)]
fn usage(mount_point: &str) -> Result<(u64, u64, u64), String> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = mount_point
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            &mut total,
            &mut total_free,
        )
    };
    if ok == 0 {
        return Err(format!(
            "GetDiskFreeSpaceExW failed (win32 error {})",
            std::io::Error::last_os_error()
        ));
    }
    Ok((total.saturating_sub(free_to_caller), total, free_to_caller))
}

/// Non-Linux/non-Windows: no native metric, so the probe reports Unreachable.
#[cfg(not(any(unix, windows)))]
fn usage(_mount_point: &str) -> Result<(u64, u64, u64), String> {
    Err("disk usage is unsupported on this platform".to_owned())
}

/// Human-readable byte count in IEC units (GiB for the probe's scale).
fn human(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource() -> Resource {
        Resource::new("root", "disk")
    }

    #[test]
    fn from_config_requires_mount_point() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"mount_point": "  "})).is_err());
    }

    #[test]
    fn from_config_reads_thresholds() {
        let probe =
            from_config(&json!({"mount_point": "/", "warn_pct": 85, "crit_pct": 95})).unwrap();
        assert_eq!(probe.threshold.warn_pct, 85);
        assert_eq!(probe.threshold.crit_pct, 95);
    }

    #[cfg(unix)]
    #[test]
    fn missing_mount_point_is_unreachable() {
        let probe = DiskProbe {
            mount_point: "/definitely/not/a/real/mount".into(),
            threshold: PctThreshold {
                warn_pct: 80,
                crit_pct: 90,
            },
        };
        let report = probe.probe(&resource());
        assert_eq!(report.status, HealthStatus::Unreachable);
        assert!(!report.succeeded);
    }

    #[cfg(unix)]
    #[test]
    fn root_fs_is_healthy_or_degraded_never_crashes() {
        // `/` always exists; the probe must return a well-formed report.
        let probe = DiskProbe {
            mount_point: "/".into(),
            threshold: PctThreshold {
                warn_pct: 80,
                crit_pct: 90,
            },
        };
        let report = probe.probe(&resource());
        assert!(
            report.observation.contains("used"),
            "{}",
            report.observation
        );
        // The status is a real health classification, never Unknown.
        assert_ne!(report.status, HealthStatus::Unknown);
    }

    #[test]
    fn human_formats_gib() {
        assert_eq!(human(0), "0.0 GiB");
        assert_eq!(human(1024 * 1024 * 1024), "1.0 GiB");
    }
}

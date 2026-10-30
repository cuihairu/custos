//! Log file keyword probe (survey §26.11, implementation order #6).
//!
//! Reads only the *new* bytes appended since the previous probe call
//! (incremental offset) and matches a regex against them; a match is
//! Unhealthy — a high-value self-healing trigger (panic/OOM in a log).
//! Rotation is detected by inode change (Unix) or by the file shrinking
//! below the tracked offset (any platform, covers truncate/rotate).
//!
//! The first probe call establishes the offset at the current end of file
//! without matching, so historical log content never alarms on startup.

use crate::config::{optional_str, required_str, u64_or};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use regex::Regex;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

const DEFAULT_MAX_BYTES_PER_CHECK: u64 = 64 * 1024;

/// Per-file monitoring state (offset + rotation identity).
#[derive(Debug, Clone, Copy, Default)]
struct LogState {
    inode: u64,
    offset: u64,
    /// False until the first probe establishes the baseline offset.
    initialized: bool,
}

/// Probes a log file for new lines matching a regex.
#[derive(Debug)]
pub struct LogProbe {
    pub path: String,
    pub pattern: Regex,
    pub max_bytes_per_check: u64,
    state: Mutex<LogState>,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.11).
pub fn from_config(config: &serde_json::Value) -> Result<LogProbe, String> {
    let path = required_str(config, "path").ok_or("`probe.path` is required for log probes")?;
    if path.trim().is_empty() {
        return Err("`probe.path` must not be empty".to_owned());
    }
    let pattern =
        optional_str(config, "pattern").ok_or("`probe.pattern` is required for log probes")?;
    let pattern =
        Regex::new(&pattern).map_err(|err| format!("invalid `probe.pattern` regex: {err}"))?;
    Ok(LogProbe {
        path,
        pattern,
        max_bytes_per_check: {
            let bytes = u64_or(config, "max_bytes_per_check", DEFAULT_MAX_BYTES_PER_CHECK);
            if bytes > 0 {
                bytes
            } else {
                DEFAULT_MAX_BYTES_PER_CHECK
            }
        },
        state: Mutex::new(LogState::default()),
    })
}

impl Probe for LogProbe {
    fn name(&self) -> &'static str {
        "log"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };

        let metadata = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot stat {}: {err}", self.path),
                );
            }
        };

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // First call: baseline at the current end of file, no matching.
        if !state.initialized {
            state.inode = inode_of(&metadata);
            state.offset = metadata.len();
            state.initialized = true;
            return report(
                true,
                HealthStatus::Healthy,
                format!(
                    "monitoring {} from offset {} (baseline set)",
                    self.path, state.offset
                ),
            );
        }

        // Rotation/truncation: inode change (Unix) or the file shrank below
        // the tracked offset (any platform) — restart from the top.
        if inode_of(&metadata) != state.inode || metadata.len() < state.offset {
            state.inode = inode_of(&metadata);
            state.offset = 0;
        }

        let new_bytes = match read_new(&self.path, state.offset, self.max_bytes_per_check) {
            Ok(bytes) => bytes,
            Err(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot read {}: {err}", self.path),
                );
            }
        };
        state.offset = metadata.len();

        if new_bytes.is_empty() {
            return report(true, HealthStatus::Healthy, "no new log lines".to_owned());
        }

        let text = String::from_utf8_lossy(&new_bytes);
        if let Some(matched) = self.pattern.find(&text) {
            let line = text.lines().find(|line| line.contains(matched.as_str()));
            let sample = line.unwrap_or(matched.as_str());
            return report(
                false,
                HealthStatus::Unhealthy,
                format!(
                    "pattern matched new log content ({} bytes, {matched:?}): {:.120}",
                    new_bytes.len(),
                    sample.trim()
                ),
            );
        }

        report(
            true,
            HealthStatus::Healthy,
            format!("{} new bytes, no match", new_bytes.len()),
        )
    }
}

/// Reads up to `max_bytes` starting at `offset`, returning what is there.
fn read_new(path: &str, offset: u64, max_bytes: u64) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|err| err.to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut limited = (&mut file).take(limit as u64);
    limited
        .read_to_end(&mut buf)
        .map_err(|err| err.to_string())?;
    Ok(buf)
}

/// File identity for rotation detection; inode on Unix, 0 elsewhere (the
/// shrink-below-offset check then covers rotation).
#[cfg(unix)]
fn inode_of(metadata: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.ino()
}

#[cfg(not(unix))]
fn inode_of(_metadata: &Metadata) -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn resource() -> Resource {
        Resource::new("svc-log", "log")
    }

    /// A temp file with `content`; returns (path, cleanup guard).
    fn temp_log(content: &str) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.log");
        std::fs::write(&path, content).unwrap();
        (path.to_string_lossy().into_owned(), dir)
    }

    fn probe_for(path: &str, pattern: &str) -> LogProbe {
        LogProbe {
            path: path.into(),
            pattern: Regex::new(pattern).unwrap(),
            max_bytes_per_check: DEFAULT_MAX_BYTES_PER_CHECK,
            state: Mutex::new(LogState::default()),
        }
    }

    #[test]
    fn from_config_requires_path_and_pattern() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"path": "/tmp/x.log"})).is_err());
        assert!(from_config(&json!({"pattern": "oops"})).is_err());
    }

    #[test]
    fn from_config_rejects_bad_regex() {
        assert!(from_config(&json!({"path": "/tmp/x.log", "pattern": "("})).is_err());
    }

    #[test]
    fn first_probe_baselines_without_alarming() {
        let (path, _dir) = temp_log("old panic happened\n");
        let probe = probe_for(&path, "panic");
        // Baseline: existing content is not matched.
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("baseline"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn new_matching_line_is_unhealthy() {
        let (path, _dir) = temp_log("");
        let probe = probe_for(&path, "panic");
        probe.probe(&resource()); // baseline at offset 0

        // Append a panic line.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "thread 'main' panicked at src/main.rs:3").unwrap();
        drop(file);

        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Unhealthy,
            "{}",
            report.observation
        );
        assert!(!report.succeeded);
        assert!(
            report.observation.contains("panic"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn non_matching_new_line_stays_healthy() {
        let (path, _dir) = temp_log("");
        let probe = probe_for(&path, "panic");
        probe.probe(&resource());

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "all good, request served").unwrap();
        drop(file);

        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
    }

    #[test]
    fn same_line_is_not_re_matched_across_calls() {
        let (path, _dir) = temp_log("");
        let probe = probe_for(&path, "panic");
        probe.probe(&resource());

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "panic once").unwrap();
        drop(file);

        let first = probe.probe(&resource());
        assert_eq!(first.status, HealthStatus::Unhealthy);

        // No new content: the same line must not re-trigger.
        let second = probe.probe(&resource());
        assert_eq!(
            second.status,
            HealthStatus::Healthy,
            "{}",
            second.observation
        );
    }

    #[test]
    fn rotation_resets_offset_and_re_reads() {
        let (path, dir) = temp_log("");
        let probe = probe_for(&path, "panic");
        probe.probe(&resource());

        // Append content, then rotate the file (rename + create fresh).
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "pre-rotation").unwrap();
        drop(file);
        probe.probe(&resource()); // advances offset past "pre-rotation"

        std::fs::rename(&path, dir.path().join("service.log.1")).unwrap();
        std::fs::write(&path, "panic after rotation\n").unwrap();

        // Inode changed (new file) → re-read from 0 → match.
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Unhealthy,
            "{}",
            report.observation
        );
    }

    #[test]
    fn missing_file_is_unreachable() {
        let probe = probe_for("/no/such/file.log", "panic");
        let report = probe.probe(&resource());
        assert_eq!(report.status, HealthStatus::Unreachable);
    }
}

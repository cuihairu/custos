//! Power policy probes (todo.md power probe family, 2026-11-22 user order):
//! `power-sleep` reports whether anything prevents this host from
//! suspending, `power-wake` reports the next scheduled wake, and
//! `power-downtime` reports whether the current time falls inside an
//! allowed sleep window.
//!
//! Probes are observation-only and never change power policy. Platform
//! support is declared by the readers here (Linux systemd + RTC, Windows
//! powercfg, macOS pmset); a host that exposes none of those surfaces is
//! `Unreachable`, never guessed. The high-risk action counterparts (mask
//! sleep targets, arm an RTC alarm, block sleep outside a window) live
//! behind the toolbox permission gate plus a second confirm token, not in
//! this module.

use crate::config::{required_str, u64_or};
use crate::exec::{run_controlled, ExecResult};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 3_000;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// `powercfg /q` subsetting GUID for the standby (sleep) idle timeout —
/// stable across UI locales, unlike the friendly name beside it.
const POWERCFG_STANDBY_TIMEOUT_GUID: &str = "29f6c1db-86da-48c5-9fdb-f2b67b1f44da";

/// Whether a host may suspend, as observed from its sleep policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SleepPolicy {
    /// `true` when sleep targets are masked/disabled or an active inhibitor
    /// blocks sleep; `false` when nothing was found that prevents it.
    pub sleep_blocked: bool,
    pub detail: String,
}

/// Parses `systemctl is-enabled` output for one sleep target. `masked`
/// blocks that target; the other recognized states do not. An unrecognized
/// value is `None` so the caller reports it instead of guessing.
pub fn parse_systemd_target_state(value: &str) -> Option<bool> {
    match value.trim() {
        "masked" | "masked-runtime" => Some(true),
        "static" | "enabled" | "enabled-runtime" | "generated" | "transient" | "disabled" => {
            Some(false)
        }
        _ => None,
    }
}

/// Parses one systemd inhibitor descriptor (`/run/systemd/inhibit/<n>`).
/// The file is marked "private, do not parse"; we read only the `WHAT` and
/// `MODE` fields to count blocking locks — never the private PID/WHO data.
pub fn parse_inhibitor_descriptor(text: &str) -> Option<(String, String)> {
    let mut what = None;
    let mut mode = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "WHAT" => what = Some(value.trim().to_owned()),
            "MODE" => mode = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    Some((what?, mode?))
}

/// True when any parsed inhibitor blocks sleep. A `delay` lock only
/// postpones the suspend briefly; only a `block` lock prevents it.
pub fn inhibitor_blocks_sleep(entries: &[(String, String)]) -> bool {
    entries
        .iter()
        .any(|(what, mode)| what == "sleep" && mode == "block")
}

/// True when the line carries a `8-4-4-4-12` hex GUID — the
/// locale-independent marker of `powercfg /q` setting and subsetting lines.
fn line_has_guid(line: &str) -> bool {
    let shape_ok = |run: &str| -> bool {
        let parts: Vec<&str> = run.split('-').collect();
        [8usize, 4, 4, 4, 12]
            .iter()
            .zip(parts.iter())
            .all(|(len, part)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
            && parts.len() == 5
    };
    let mut run = String::new();
    for c in line.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_hexdigit() || c == '-' {
            run.push(c);
        } else {
            if shape_ok(&run) {
                return true;
            }
            run.clear();
        }
    }
    false
}

/// Parses the 0x hex settings that follow the standby-timeout subsetting
/// GUID in `powercfg /q` output. All-zero indices mean sleep is disabled;
/// any non-zero idle timer means the host will suspend. Locale-independent
/// by design: GUID lines and 0x tokens are identical on every UI language,
/// only the friendly names around them translate.
pub fn parse_powercfg_sleep_disabled(output: &str) -> Option<bool> {
    let mut in_standby_block = false;
    let mut values: Vec<u64> = Vec::new();
    for line in output.lines() {
        if line_has_guid(line) {
            in_standby_block = line.contains(POWERCFG_STANDBY_TIMEOUT_GUID);
            continue;
        }
        if !in_standby_block {
            continue;
        }
        for token in line.split_whitespace() {
            if let Some(hex) = token.strip_prefix("0x") {
                if let Ok(value) = u64::from_str_radix(hex, 16) {
                    values.push(value);
                }
            }
        }
    }
    if values.is_empty() {
        return None;
    }
    Some(values.iter().all(|v| *v == 0))
}

/// Parses `pmset -g` output: `disablesleep 1` blocks sleep on macOS.
pub fn parse_pmset_sleep_disabled(output: &str) -> Option<bool> {
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("disablesleep") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u8>().ok())
                .map(|v| v == 1);
        }
    }
    None
}

/// Parses `pmset -g sched` output: any `Scheduled power on|wake` line
/// schedules a wake. Returns the raw scheduled text so the observation
/// shows exactly what the host promised, never an invented epoch.
/// `Repeating poweron:` alone is a recurrence header without a time, so it
/// does not count as an armed wake.
pub fn parse_pmset_sched_wake(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| {
            (line.starts_with("Scheduled power on") || line.starts_with("Scheduled wake"))
                && line.contains(" at ")
        })
        .map(str::to_owned)
}

/// Reads the Linux sleep policy from systemd targets and inhibitor locks.
/// A host without systemd is an error the caller reports as Unreachable.
#[cfg(target_os = "linux")]
pub fn linux_sleep_policy() -> Result<SleepPolicy, String> {
    if !std::path::Path::new("/run/systemd/system").exists() {
        return Err("systemd is not present on this host".into());
    }
    let mut blocked = false;
    let mut detail = Vec::new();
    for target in ["sleep.target", "suspend.target"] {
        let argv = ["systemctl", "is-enabled", target].map(String::from);
        match run_controlled(
            &argv,
            Duration::from_millis(DEFAULT_TIMEOUT_MS),
            MAX_OUTPUT_BYTES,
        ) {
            ExecResult::Succeeded { stdout, .. } => match parse_systemd_target_state(&stdout) {
                Some(is_masked) => {
                    blocked |= is_masked;
                    detail.push(format!("{target}={}", stdout.trim()));
                }
                None => return Err(format!("unrecognized state of {target}: {}", stdout.trim())),
            },
            other => {
                return Err(format!(
                    "systemctl is-enabled {target}: {}",
                    other.observation()
                ))
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir("/run/systemd/inhibit") {
        let mut inhibitors = Vec::new();
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().ends_with(".ref") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if let Some(lock) = parse_inhibitor_descriptor(&text) {
                    inhibitors.push(lock);
                }
            }
        }
        let blocking = inhibitors
            .iter()
            .filter(|(what, mode)| what == "sleep" && mode == "block")
            .count();
        if blocking > 0 {
            blocked = true;
            detail.push(format!("{blocking} blocking sleep inhibitor(s)"));
        }
    }
    Ok(SleepPolicy {
        sleep_blocked: blocked,
        detail: if detail.is_empty() {
            "no sleep-prevention found".into()
        } else {
            detail.join("; ")
        },
    })
}

/// Windows sleep policy via `powercfg /q SCHEME_CURRENT SUB_SLEEP`: a zero
/// standby idle timeout on every index means sleep is disabled.
#[cfg(windows)]
pub fn windows_sleep_policy(timeout: Duration) -> Result<SleepPolicy, String> {
    let argv = ["powercfg", "/q", "SCHEME_CURRENT", "SUB_SLEEP"].map(String::from);
    match run_controlled(&argv, timeout, MAX_OUTPUT_BYTES) {
        ExecResult::Succeeded { stdout, .. } => match parse_powercfg_sleep_disabled(&stdout) {
            Some(disabled) => Ok(SleepPolicy {
                sleep_blocked: disabled,
                detail: format!("powercfg SUB_SLEEP standby timeout zero={disabled}"),
            }),
            None => Err("powercfg output carried no standby timeout setting".into()),
        },
        other => Err(format!("powercfg /q SUB_SLEEP: {}", other.observation())),
    }
}

/// macOS sleep policy via `pmset -g`: `disablesleep 1` blocks sleep.
#[cfg(target_os = "macos")]
pub fn macos_sleep_policy(timeout: Duration) -> Result<SleepPolicy, String> {
    let argv = ["pmset", "-g"].map(String::from);
    match run_controlled(&argv, timeout, MAX_OUTPUT_BYTES) {
        ExecResult::Succeeded { stdout, .. } => match parse_pmset_sleep_disabled(&stdout) {
            Some(disabled) => Ok(SleepPolicy {
                sleep_blocked: disabled,
                detail: format!("pmset disablesleep={disabled}"),
            }),
            None => Err("pmset -g output carried no disablesleep line".into()),
        },
        other => Err(format!("pmset -g: {}", other.observation())),
    }
}

/// Reads this host's sleep policy; the platform dispatch is explicit and
/// unsupported platforms are errors, never guesses.
pub fn sleep_policy(_timeout: Duration) -> Result<SleepPolicy, String> {
    #[cfg(target_os = "linux")]
    {
        linux_sleep_policy()
    }
    #[cfg(windows)]
    {
        windows_sleep_policy(_timeout)
    }
    #[cfg(target_os = "macos")]
    {
        macos_sleep_policy(_timeout)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        Err("power-sleep is not supported on this platform".into())
    }
}

/// One `power-sleep` probe: Healthy when something prevents suspension
/// (the fleet's wanted state), Unhealthy when the host may sleep freely.
#[derive(Debug, Clone)]
pub struct SleepProbe {
    pub timeout: Duration,
}

pub fn sleep_from_config(config: &serde_json::Value) -> Result<SleepProbe, String> {
    Ok(SleepProbe {
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
    })
}

impl Probe for SleepProbe {
    fn name(&self) -> &'static str {
        "power-sleep"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |status, succeeded, observation| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        match sleep_policy(self.timeout) {
            Ok(policy) => {
                let blocked = policy.sleep_blocked;
                report(
                    if blocked {
                        HealthStatus::Healthy
                    } else {
                        HealthStatus::Unhealthy
                    },
                    blocked,
                    if blocked {
                        format!("sleep prevented: {}", policy.detail)
                    } else {
                        format!("nothing prevents sleep: {}", policy.detail)
                    },
                )
            }
            Err(reason) => report(
                HealthStatus::Unreachable,
                false,
                format!("cannot determine sleep policy: {reason}"),
            ),
        }
    }
}

/// Parses `/sys/class/rtc/rtc0/wakealarm` (unix seconds, 0 or an empty
/// file = none armed; some kernels — e.g. the alarmtimer under KVM — show
/// an empty file instead of `0` when no alarm is set).
pub fn parse_wakealarm(text: &str) -> Option<u64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(0);
    }
    trimmed.parse::<u64>().ok()
}

/// Next scheduled wake as observed on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeSchedule {
    pub armed: bool,
    pub at_unix_secs: Option<u64>,
    pub detail: String,
}

/// Reads the Linux RTC wake alarm. A missing RTC or unreadable alarm file
/// is an error; a zero alarm means "no wake scheduled".
#[cfg(target_os = "linux")]
pub fn linux_wake_schedule() -> Result<WakeSchedule, String> {
    let path = "/sys/class/rtc/rtc0/wakealarm";
    let text = std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))?;
    let alarm = parse_wakealarm(&text)
        .ok_or_else(|| format!("unrecognized wakealarm value: {}", text.trim()))?;
    Ok(WakeSchedule {
        armed: alarm > 0,
        at_unix_secs: (alarm > 0).then_some(alarm),
        detail: if alarm > 0 {
            format!("rtc0 wakealarm={alarm}")
        } else {
            "rtc0 wakealarm=0 (no wake scheduled)".into()
        },
    })
}

/// macOS wake schedule via `pmset -g sched`.
#[cfg(target_os = "macos")]
pub fn macos_wake_schedule(timeout: Duration) -> Result<WakeSchedule, String> {
    let argv = ["pmset", "-g", "sched"].map(String::from);
    match run_controlled(&argv, timeout, MAX_OUTPUT_BYTES) {
        ExecResult::Succeeded { stdout, .. } => match parse_pmset_sched_wake(&stdout) {
            Some(line) => Ok(WakeSchedule {
                // pmset prints a local datetime, not an epoch; the raw
                // promise is reported verbatim rather than guessed into
                // seconds.
                armed: true,
                at_unix_secs: None,
                detail: line,
            }),
            None => Ok(WakeSchedule {
                armed: false,
                at_unix_secs: None,
                detail: "pmset -g sched: no scheduled wake/poweron".into(),
            }),
        },
        other => Err(format!("pmset -g sched: {}", other.observation())),
    }
}

/// Reads this host's wake schedule; explicit platform dispatch, no guesses.
pub fn wake_schedule(_timeout: Duration) -> Result<WakeSchedule, String> {
    #[cfg(target_os = "linux")]
    {
        linux_wake_schedule()
    }
    #[cfg(target_os = "macos")]
    {
        macos_wake_schedule(_timeout)
    }
    #[cfg(windows)]
    {
        // The schtasks wake bit has no stable locale-independent query
        // surface; arming it programmatically is equally out of reach.
        // Declared unsupported rather than guessed (§11.1).
        Err(
            "power-wake on Windows is not wired (schtasks wake bit has no \
             locale-stable parse surface); use an RTC-equipped Linux host"
                .into(),
        )
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        Err("power-wake is not supported on this platform".into())
    }
}

/// One `power-wake` probe: Healthy when a wake is scheduled (a suspended
/// fleet host will come back), Unhealthy when none is armed.
#[derive(Debug, Clone)]
pub struct WakeProbe {
    pub timeout: Duration,
}

pub fn wake_from_config(config: &serde_json::Value) -> Result<WakeProbe, String> {
    Ok(WakeProbe {
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
    })
}

impl Probe for WakeProbe {
    fn name(&self) -> &'static str {
        "power-wake"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |status, succeeded, observation| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        match wake_schedule(self.timeout) {
            Ok(schedule) => {
                let armed = schedule.armed;
                report(
                    if armed {
                        HealthStatus::Healthy
                    } else {
                        HealthStatus::Unhealthy
                    },
                    armed,
                    schedule.detail,
                )
            }
            Err(reason) => report(
                HealthStatus::Unreachable,
                false,
                format!("cannot determine wake schedule: {reason}"),
            ),
        }
    }
}

/// Parses one `HH:MM` token into minutes since midnight.
pub fn parse_hhmm(value: &str) -> Option<u32> {
    let (hh, mm) = value.trim().split_once(':')?;
    let hh = hh.parse::<u32>().ok()?;
    let mm = mm.parse::<u32>().ok()?;
    (hh < 24 && mm < 60).then_some(hh * 60 + mm)
}

/// True when `now_minutes` falls in [start, end); equal bounds mean the
/// window covers the whole day, and a start past the end wraps midnight.
pub fn within_window(now_minutes: u32, start_minutes: u32, end_minutes: u32) -> bool {
    if start_minutes == end_minutes {
        return true;
    }
    if start_minutes < end_minutes {
        now_minutes >= start_minutes && now_minutes < end_minutes
    } else {
        now_minutes >= start_minutes || now_minutes < end_minutes
    }
}

/// Current local time as minutes since midnight via `date +%H:%M` (a
/// controlled command, never a shell), so the clock source is explicit.
pub fn current_local_minutes(timeout: Duration) -> Result<u32, String> {
    let argv = ["date", "+%H:%M"].map(String::from);
    match run_controlled(&argv, timeout, MAX_OUTPUT_BYTES) {
        ExecResult::Succeeded { stdout, .. } => parse_hhmm(&stdout)
            .ok_or_else(|| format!("unrecognized date output: {}", stdout.trim())),
        other => Err(format!("date +%H:%M: {}", other.observation())),
    }
}

/// One `power-downtime` probe: Healthy while the current time is inside the
/// allowed sleep window (a suspend now is policy-legal), Unhealthy outside.
#[derive(Debug, Clone)]
pub struct DowntimeProbe {
    pub start: String,
    pub end: String,
    pub timeout: Duration,
}

pub fn downtime_from_config(config: &serde_json::Value) -> Result<DowntimeProbe, String> {
    let start = required_str(config, "start")
        .ok_or("`probe.start` is required for power-downtime resources")?;
    let end = required_str(config, "end")
        .ok_or("`probe.end` is required for power-downtime resources")?;
    parse_hhmm(&start)
        .ok_or_else(|| format!("invalid `probe.start` value `{start}` (want HH:MM)"))?;
    parse_hhmm(&end).ok_or_else(|| format!("invalid `probe.end` value `{end}` (want HH:MM)"))?;
    Ok(DowntimeProbe {
        start,
        end,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
    })
}

impl Probe for DowntimeProbe {
    fn name(&self) -> &'static str {
        "power-downtime"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |status, succeeded, observation| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let now = match current_local_minutes(self.timeout) {
            Ok(now) => now,
            Err(reason) => {
                return report(
                    HealthStatus::Unreachable,
                    false,
                    format!("cannot read local time: {reason}"),
                )
            }
        };
        let (start, end) = (
            parse_hhmm(&self.start).unwrap_or(0),
            parse_hhmm(&self.end).unwrap_or(0),
        );
        let inside = within_window(now, start, end);
        report(
            if inside {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unhealthy
            },
            inside,
            format!(
                "local {:02}:{:02} is {} the sleep window {}-{}",
                now / 60,
                now % 60,
                if inside { "inside" } else { "outside" },
                self.start,
                self.end
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_target_states_distinguish_masked_from_normal() {
        assert_eq!(parse_systemd_target_state("masked\n"), Some(true));
        assert_eq!(parse_systemd_target_state("masked-runtime"), Some(true));
        assert_eq!(parse_systemd_target_state("static"), Some(false));
        assert_eq!(parse_systemd_target_state("enabled-runtime"), Some(false));
        assert_eq!(parse_systemd_target_state("disabled"), Some(false));
        assert_eq!(parse_systemd_target_state("garbage"), None);
    }

    #[test]
    fn inhibitor_descriptor_skips_comment_and_private_lines() {
        // The real file starts with the "do not parse" comment and carries
        // PID/WHO/WHY fields; only WHAT/MODE are read.
        let text = "# This is private data. Do not parse.\n\
                    WHAT=sleep\n\
                    MODE=delay\n\
                    UID=0\n\
                    PID=1529\n\
                    WHO=ModemManager\n\
                    WHY=\"ModemManager needs to reset devices\"\n";
        assert_eq!(
            parse_inhibitor_descriptor(text),
            Some(("sleep".to_owned(), "delay".to_owned()))
        );
        assert_eq!(parse_inhibitor_descriptor("no keys at all\n"), None);
    }

    #[test]
    fn delay_inhibitor_does_not_block_sleep_but_block_does() {
        assert!(!inhibitor_blocks_sleep(&[("sleep".into(), "delay".into())]));
        assert!(inhibitor_blocks_sleep(&[("sleep".into(), "block".into())]));
        assert!(!inhibitor_blocks_sleep(&[(
            "shutdown".into(),
            "block".into()
        )]));
    }

    #[test]
    fn powercfg_guid_block_is_locale_independent() {
        // English locale: subsetting header with friendly name, then the
        // two index lines. zh-CN prints the same GUIDs and 0x tokens.
        let disabled = "\
    Subgroup GUID: 238c9fa8-0aad-41ed-83f4-97be242c8f20  (Sleep)
      Power Setting GUID: 29f6c1db-86da-48c5-9fdb-f2b67b1f44da  (Sleep after)
        Current AC Power Setting Index: 0x00000000
        Current DC Power Setting Index: 0x00000000\n";
        assert_eq!(parse_powercfg_sleep_disabled(disabled), Some(true));

        let enabled = "\
    Subgroup GUID: 238c9fa8-0aad-41ed-83f4-97be242c8f20  (Sleep)
      Power Setting GUID: 29f6c1db-86da-48c5-9fdb-f2b67b1f44da  (Sleep after)
        Current AC Power Setting Index: 0x00000384
        Current DC Power Setting Index: 0x00000384\n";
        assert_eq!(parse_powercfg_sleep_disabled(enabled), Some(false));

        // zh-CN fixture: same structure, localized headers (fixture from
        // the Windows-suite discipline: real bytes, GUIDs unchanged).
        let zh = "\
    子组 GUID: 238c9fa8-0aad-41ed-83f4-97be242c8f20  (睡眠)
      电源设置 GUID: 29f6c1db-86da-48c5-9fdb-f2b67b1f44da  (在此时间后睡眠)
        当前交流电源设置索引: 0x00000000
        当前直流电源设置索引: 0x00000000\n";
        assert_eq!(parse_powercfg_sleep_disabled(zh), Some(true));

        // The standby subsetting absent: nothing to judge.
        let absent = "\
    Subgroup GUID: 238c9fa8-0aad-41ed-83f4-97be242c8f20  (Sleep)
      Power Setting GUID: 29f6c1db-0000-0000-0000-000000000000  (Other)\n";
        assert_eq!(parse_powercfg_sleep_disabled(absent), None);
    }

    #[test]
    fn pmset_fixtures() {
        assert_eq!(
            parse_pmset_sleep_disabled(" disablesleep        1\n"),
            Some(true)
        );
        assert_eq!(
            parse_pmset_sleep_disabled(" disablesleep        0\n"),
            Some(false)
        );
        assert_eq!(
            parse_pmset_sleep_disabled(" sleep                30\n"),
            None
        );

        // `Repeating poweron:` is a recurrence header without a time —
        // not an armed wake. `Scheduled power on at <time>` is one.
        assert_eq!(
            parse_pmset_sched_wake("Repeating poweron at 08:00:00 every day\n"),
            None
        );
        assert_eq!(
            parse_pmset_sched_wake("Scheduled power on at 2026/11/26 08:00:00\n"),
            Some("Scheduled power on at 2026/11/26 08:00:00".to_owned())
        );
        assert_eq!(
            parse_pmset_sched_wake("Scheduled wake at 2026/11/26 08:00:00\n"),
            Some("Scheduled wake at 2026/11/26 08:00:00".to_owned())
        );
        assert_eq!(parse_pmset_sched_wake(" sleep 30\n"), None);
    }

    #[test]
    fn wakealarm_parses_seconds_and_zero() {
        assert_eq!(parse_wakealarm("0\n"), Some(0));
        assert_eq!(parse_wakealarm("1764000000\n"), Some(1_764_000_000));
        assert_eq!(parse_wakealarm("garbage"), None);
    }

    #[test]
    fn hhmm_windows_handle_wrap_over_midnight() {
        assert_eq!(parse_hhmm("23:30"), Some(1410));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert!(within_window(1410, 1380, 420), "23:30 in 23:00-07:00");
        assert!(within_window(60, 1380, 420), "01:00 in 23:00-07:00");
        assert!(!within_window(720, 1380, 420), "12:00 outside");
        assert!(within_window(720, 0, 0), "equal bounds = whole day");
    }

    #[test]
    fn downtime_config_rejects_malformed_times() {
        let bad = serde_json::json!({"start": "25:00", "end": "07:00"});
        assert!(downtime_from_config(&bad).is_err());
        let missing = serde_json::json!({"start": "23:00"});
        assert!(downtime_from_config(&missing).is_err());
        let good = serde_json::json!({"start": "23:00", "end": "07:00"});
        assert!(downtime_from_config(&good).is_ok());
    }

    #[test]
    fn local_minutes_reads_the_host_clock() {
        // Live but harmless: `date +%H:%M` must yield a valid value on any
        // host with coreutils/BSD date present.
        if let Ok(minutes) = current_local_minutes(Duration::from_millis(2_000)) {
            assert!(minutes < 24 * 60);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_sleep_policy_reports_a_real_state() {
        // Live evidence (todo.md acceptance): on a systemd host the policy
        // reader must return some real verdict, never guess. On this dev
        // machine sleep.target/suspend.target are `static` and ModemManager
        // holds only a `delay` lock, so nothing blocks sleep.
        if std::path::Path::new("/run/systemd/system").exists() {
            let policy = linux_sleep_policy().expect("systemd present: policy readable");
            assert!(
                policy.detail.contains("sleep.target"),
                "detail carries target states: {}",
                policy.detail
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_wake_schedule_reads_the_real_rtc() {
        // Live evidence (todo.md acceptance): the RTC file must parse into
        // a schedule even when no alarm is armed.
        if std::path::Path::new("/sys/class/rtc/rtc0/wakealarm").exists() {
            let schedule = linux_wake_schedule().expect("rtc0 present: schedule readable");
            assert!(schedule.detail.starts_with("rtc0 wakealarm="));
        }
    }

    // ---- todo.md acceptance end-to-end (run by hand on a systemd host,
    // hence #[ignore]; needs root because /run/systemd/inhibit and the RTC
    // write path are root-only):
    //   cargo test -p custos-probe power -- --ignored --nocapture
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn live_inhibitor_lock_flips_sleep_policy() {
        use std::process::{Command, Stdio};
        if !std::path::Path::new("/run/systemd/system").exists() {
            return;
        }
        // A real blocking inhibitor held by a child process.
        let mut child = Command::new("systemd-inhibit")
            .args(["--what", "sleep", "--mode", "block", "sleep", "30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("systemd-inhibit available");
        std::thread::sleep(Duration::from_millis(500));
        let policy = linux_sleep_policy().expect("systemd host");
        assert!(
            policy.sleep_blocked,
            "a real block lock must flip the policy, got: {}",
            policy.detail
        );
        assert!(
            policy.detail.contains("blocking sleep inhibitor"),
            "detail names the inhibitor: {}",
            policy.detail
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn live_rtc_wakealarm_write_and_clear() {
        const PATH: &str = "/sys/class/rtc/rtc0/wakealarm";
        if !std::path::Path::new(PATH).exists() {
            return;
        }
        // Arm ~120s out (root-only write), verify it reads back armed, then
        // clear and verify it is clear — todo.md acceptance ②.
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs()
            + 120;
        std::fs::write(PATH, format!("{at}\n")).expect("write wakealarm (needs root)");
        let armed = linux_wake_schedule().expect("read back");
        assert!(armed.armed, "alarm must read back armed: {}", armed.detail);
        assert_eq!(armed.at_unix_secs, Some(at), "epoch round-trip");
        std::fs::write(PATH, "0\n").expect("clear wakealarm");
        let cleared = linux_wake_schedule().expect("read cleared");
        assert!(
            !cleared.armed,
            "alarm must be clearable: {}",
            cleared.detail
        );
        assert_eq!(cleared.at_unix_secs, None);
    }
}

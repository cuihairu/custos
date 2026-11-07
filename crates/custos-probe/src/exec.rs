//! Controlled command execution shared by the Exec Probe and builtin
//! recovery actions (docs/guide/probe-diagnosis.md §5.4, §4.3).
//!
//! Constraints: runtime limit, output size limit, process-tree cleanup on
//! timeout (Windows must not leave orphaned children), explicit errors for
//! startup failures.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Result of one controlled execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecResult {
    /// Exit code 0. `truncated` marks output clipped at the size limit.
    Succeeded { stdout: String, truncated: bool },
    /// Non-zero exit code.
    Failed {
        code: i32,
        stdout: String,
        stderr: String,
        truncated: bool,
    },
    /// Deadline hit; the process tree was killed.
    TimedOut,
    /// The command could not be started at all.
    StartFailed(String),
}

impl ExecResult {
    pub fn succeeded(&self) -> bool {
        matches!(self, ExecResult::Succeeded { .. })
    }

    /// One-line observation for evidence records.
    pub fn observation(&self) -> String {
        match self {
            ExecResult::Succeeded { stdout, truncated } => {
                if *truncated {
                    format!("exit 0, output truncated: {}", first_line(stdout))
                } else {
                    format!("exit 0: {}", first_line(stdout))
                }
            }
            ExecResult::Failed {
                code,
                stderr,
                truncated,
                ..
            } => {
                if *truncated {
                    format!("exit {code}, output truncated: {}", first_line(stderr))
                } else {
                    format!("exit {code}: {}", first_line(stderr))
                }
            }
            ExecResult::TimedOut => "timed out".to_owned(),
            ExecResult::StartFailed(err) => format!("start failed: {err}"),
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

/// Runs `argv` with a hard runtime limit and output cap, killing the whole
/// process tree on timeout.
pub fn run_controlled(argv: &[String], timeout: Duration, max_output_bytes: usize) -> ExecResult {
    if argv.is_empty() {
        return ExecResult::StartFailed("empty command".into());
    }
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // New process group so the whole tree can be killed on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return ExecResult::StartFailed(err.to_string()),
    };

    // Reader threads drain both pipes to EOF so the child can never block on
    // a full pipe; each keeps at most `max_output_bytes` and a truncation flag.
    let stdout_reader = child
        .stdout
        .take()
        .map(|mut pipe| std::thread::spawn(move || read_capped(&mut pipe, max_output_bytes)));
    let stderr_reader = child
        .stderr
        .take()
        .map(|mut pipe| std::thread::spawn(move || read_capped(&mut pipe, max_output_bytes)));

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = join_reader(stdout_reader);
                let stderr = join_reader(stderr_reader);
                return match status.code() {
                    Some(0) => ExecResult::Succeeded {
                        stdout: stdout.0,
                        truncated: stdout.1,
                    },
                    Some(code) => ExecResult::Failed {
                        code,
                        stdout: stdout.0,
                        stderr: stderr.0,
                        truncated: stdout.1 || stderr.1,
                    },
                    // Killed by signal: treat as failure with signal status.
                    None => ExecResult::Failed {
                        code: -1,
                        stdout: stdout.0,
                        stderr: stderr.0,
                        truncated: stdout.1 || stderr.1,
                    },
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_tree(&child);
                    let _ = child.wait();
                    // Readers observe EOF once the tree is dead.
                    let _ = join_reader(stdout_reader);
                    let _ = join_reader(stderr_reader);
                    return ExecResult::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => {
                // The child DID spawn: a wait error must not leak it. Kill
                // the tree and join the readers exactly like the timeout
                // arm, then report; the label says start, the detail says
                // where it actually broke.
                kill_tree(&child);
                let _ = child.wait();
                let _ = join_reader(stdout_reader);
                let _ = join_reader(stderr_reader);
                return ExecResult::StartFailed(format!("wait failed after spawn: {err}"));
            }
        }
    }
}

type CappedOutput = (String, bool);

fn read_capped<R: Read>(pipe: &mut R, max_bytes: usize) -> CappedOutput {
    let mut kept = Vec::with_capacity(max_bytes.min(8 * 1024));
    let mut buf = [0u8; 4096];
    let mut truncated = false;
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = max_bytes.saturating_sub(kept.len());
                if n > room {
                    truncated = true;
                }
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    (decode_console_output(&kept), truncated)
}

/// Decodes captured command output bytes. Valid UTF-8 passes through
/// untouched; anything else is decoded with the console OEM code page of
/// the host. The previous `String::from_utf8_lossy` destroyed every
/// non-ASCII byte of a zh-CN `route print`/`netsh` output into U+FFFD
/// mush (openclaw audit 2026-11-05). Pure, so the fixture test runs on
/// every platform.
pub fn decode_console_output(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    // Encoding selection is a heuristic with one inherent ambiguity: a
    // byte sequence that happens to be valid UTF-8 (e.g. GBK 默 = C4 AC,
    // which is also U+012C) is decoded as UTF-8 rather than the OEM
    // code page when the WHOLE payload is valid UTF-8. This is accepted:
    // it only affects display fidelity of non-ASCII text, never the ASCII
    // route/NIC parsing, and the old from_utf8_lossy path had the same
    // UTF-8-first preference. BOM handling is off — console output has no
    // BOM, and sniffing one would silently strip the first bytes.
    decode_with_oem_code_page(oem_code_page(), bytes)
}

/// The host console OEM code page. Windows reports it via `GetOEMCP`;
/// other hosts have no console code page, so GBK (936) is the historical
/// default for the zh-CN deployments the probe targets.
fn oem_code_page() -> u16 {
    #[cfg(windows)]
    {
        // SAFETY: GetOEMCP is a plain kernel32 call with no arguments; it
        // cannot fail and has no precondition on its input.
        unsafe { windows_sys::Win32::Globalization::GetOEMCP() as u16 }
    }
    #[cfg(not(windows))]
    {
        // Non-Windows hosts don't run `route print`/`netsh`; keep the
        // historical zh-CN default so the fixture test stays meaningful.
        936
    }
}

/// Decodes non-UTF-8 output with the given Windows OEM code page. The
/// common single-byte DOS pages 437 and 850 use the table-driven
/// `known_codepages` decoder; every other page falls through to the
/// matching `encoding_rs` single-byte/DBCS encoding. No page fails hard:
/// bytes outside the map become U+FFFD, matching the old lossy fallback
/// for garbage. Pure, so it is tested on every platform with a code page
/// argument rather than only where `GetOEMCP` runs.
fn decode_with_oem_code_page(cp: u16, bytes: &[u8]) -> String {
    let page = known_codepages::CodePage::from_number(cp);
    if let Some(page) = page {
        let mut out = String::with_capacity(bytes.len());
        for &byte in bytes {
            out.push(page.decode(byte).unwrap_or('\u{FFFD}'));
        }
        return out;
    }
    // No single-byte/OEM encoding fails hard: bytes outside the map
    // become U+FFFD, which matches the old lossy fallback for garbage.
    let (text, _had_errors) = encoding_for_oem_code_page(cp).decode_without_bom_handling(bytes);
    text.into_owned()
}

/// Maps a Windows OEM code page to the `encoding_rs` encoding used for
/// non-UTF-8 output. Pages 437 and 850 are handled by
/// `decode_with_oem_code_page` before this is reached; unknown pages fall
/// back to GBK (the historical zh-CN default).
fn encoding_for_oem_code_page(cp: u16) -> &'static encoding_rs::Encoding {
    match cp {
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        866 => encoding_rs::IBM866,
        874 => encoding_rs::WINDOWS_874,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        _ => encoding_rs::GBK,
    }
}

fn join_reader(handle: Option<std::thread::JoinHandle<CappedOutput>>) -> CappedOutput {
    handle
        .and_then(|h| h.join().ok())
        .unwrap_or_else(|| (String::new(), false))
}

/// Kills the child and, on timeout, its whole process tree.
fn kill_tree(child: &std::process::Child) {
    #[cfg(unix)]
    {
        // process_group(0) made the child a group leader (pgid = pid);
        // kill(-pgid, SIGKILL) takes the whole tree down without orphaned
        // children. Direct syscall — spawning an external `kill` binary can
        // itself fail under CI load (fork limits), which left child.wait()
        // blocked forever and orphans behind (the 5-hour nightly hang).
        let pgid = child.id() as libc::pid_t;
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
            // Belt and braces: the pid itself, in case the group is gone.
            libc::kill(pgid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        // taskkill /T walks the child tree; /F forces termination.
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// The Exec Probe: runs a controlled check command and maps the result to a
/// probe verdict (docs/guide/probe-diagnosis.md §5.4: exit code, stdout,
/// stderr, timeout, start failure, truncation).
pub struct ExecProbe {
    pub command: Vec<String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl ExecProbe {
    pub fn from_argv(command: Vec<String>, timeout: Duration, max_output_bytes: usize) -> Self {
        Self {
            command,
            timeout,
            max_output_bytes,
        }
    }
}

impl crate::Probe for ExecProbe {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn probe(&self, resource: &custos_model::Resource) -> crate::ProbeReport {
        let result = run_controlled(&self.command, self.timeout, self.max_output_bytes);
        let status = match &result {
            ExecResult::Succeeded { .. } => custos_model::HealthStatus::Healthy,
            ExecResult::Failed { .. } => custos_model::HealthStatus::Unhealthy,
            // The probe itself could not obtain a valid result.
            ExecResult::TimedOut | ExecResult::StartFailed(_) => {
                custos_model::HealthStatus::Unreachable
            }
        };
        crate::ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded: result.succeeded(),
            status,
            observation: result.observation(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(program: &str, args: &[&str]) -> Vec<String> {
        std::iter::once(program.to_owned())
            .chain(args.iter().map(|a| a.to_string()))
            .collect()
    }

    #[test]
    fn success_captures_stdout() {
        let result = run_controlled(
            &argv("sh", &["-c", "echo ok"]),
            Duration::from_secs(5),
            4096,
        );
        assert!(matches!(&result, ExecResult::Succeeded { stdout, .. } if stdout.trim() == "ok"));
    }

    #[test]
    fn failure_captures_code_and_stderr() {
        let result = run_controlled(
            &argv("sh", &["-c", "echo boom >&2; exit 3"]),
            Duration::from_secs(5),
            4096,
        );
        match result {
            ExecResult::Failed { code, stderr, .. } => {
                assert_eq!(code, 3);
                assert!(stderr.contains("boom"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[test]
    fn timeout_kills_process_tree() {
        let start = Instant::now();
        // Unique durations so the assertion can find exactly our orphans.
        // A per-run unique duration so stale `sleep`s from earlier test
        // runs cannot pollute the orphan check.
        let marker = format!("sleep 187.{}", std::process::id());
        let result = run_controlled(
            &argv("sh", &["-c", &format!("{marker} & {marker}")]),
            Duration::from_millis(200),
            4096,
        );
        assert_eq!(result, ExecResult::TimedOut);
        // The probe timeout is 200ms; even a heavily loaded 2-core CI
        // runner returns within a few seconds. A 10s bound still catches a
        // 25s-class regression (the old 30s bound could not).
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "must not wait for the full sleep"
        );
        // Fault injection (docs/guide/testing.md §16.3): after a timeout the
        // WHOLE process tree must be gone — no orphaned children survive.
        // (pgrep exits non-zero when nothing matches.)
        let orphans = Command::new("pgrep")
            .arg("-f")
            .arg(&marker)
            .output()
            .expect("pgrep runs");
        assert!(
            !orphans.status.success(),
            "process tree survived the timeout: {}",
            String::from_utf8_lossy(&orphans.stdout)
        );
    }

    #[test]
    fn output_is_capped_and_flagged() {
        let result = run_controlled(
            &argv("sh", &["-c", "yes hello"]),
            Duration::from_secs(2),
            1024,
        );
        // `yes` never exits; it dies by timeout, but the capped output must
        // be flagged as truncated rather than unbounded.
        match result {
            ExecResult::TimedOut => {}
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[test]
    fn start_failure_is_reported() {
        let result = run_controlled(
            &argv("/nonexistent-custos-binary", &[]),
            Duration::from_secs(1),
            64,
        );
        assert!(matches!(result, ExecResult::StartFailed(_)));
    }

    #[test]
    fn output_decoding_prefers_utf8_then_gbk() {
        // Valid UTF-8 passes through byte-for-byte.
        assert_eq!(
            decode_console_output("网关 192.168.1.1".as_bytes()),
            "网关 192.168.1.1"
        );
        // zh-CN OEM code page (CP936): "默认网关" as GBK bytes. The old
        // from_utf8_lossy path turned each of these bytes into U+FFFD
        // replacement mush.
        let gbk = [0xC4u8, 0xAC, 0xC8, 0xCF, 0xCD, 0xF8, 0xB9, 0xD8];
        assert_eq!(decode_console_output(&gbk), "默认网关");
        // Mixed GBK payload + ASCII, e.g. a netsh state line.
        let mut netsh = b"State: ".to_vec();
        netsh.extend([0xD2, 0xD1, 0xC1, 0xAC, 0xBD, 0xD3]); // 已连接
        assert_eq!(decode_console_output(&netsh), "State: 已连接");
        // Bytes invalid in both UTF-8 and GBK still degrade to U+FFFD
        // rather than panicking.
        assert_eq!(decode_console_output(&[0xFF, 0xFE]), "\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn oem_cp437_and_cp850_are_not_misdecoded_as_gbk() {
        // CP437: 0x82=é, 0xE1=ß, 0x9C=£, 0xE5=σ. The old `_ => GBK`
        // fallback decoded these pairs as CJK (e.g. 0x82,0xE1 -> 傖),
        // turning English/Western European console output into garbage.
        assert_eq!(decode_with_oem_code_page(437, &[0x82, 0xE1]), "éß");
        assert_eq!(decode_with_oem_code_page(437, &[0x9C]), "£");
        assert_eq!(decode_with_oem_code_page(437, &[0xE5]), "σ");
        // CP850: 0x9B=ø, 0x9D=Ø, 0xE1=ß.
        assert_eq!(decode_with_oem_code_page(850, &[0x9B]), "ø");
        assert_eq!(decode_with_oem_code_page(850, &[0x9D]), "Ø");
        assert_eq!(decode_with_oem_code_page(850, &[0xE1]), "ß");
        // ASCII is invariant on both single-byte pages.
        assert_eq!(decode_with_oem_code_page(437, b"C:\\> "), "C:\\> ");
        // An unknown code page still falls back to GBK, preserving the
        // historical zh-CN behaviour rather than panicking.
        assert_eq!(decode_with_oem_code_page(65001, &[0xC4, 0xAC]), "默");
    }

    #[test]
    fn remaining_dos_code_pages_are_not_misdecoded_as_gbk() {
        // openclaw audit 2026-11-05: only CP437/CP850 were table-decoded;
        // every other DOS single-byte page (CP775/852/855/857/858/860-865/
        // 869) fell through to GBK. With the `all` known-codepages feature
        // each page now decodes its own table, so a byte that GBK would
        // turn into a CJK pair stays a single Latin/Cyrillic character.
        assert_eq!(decode_with_oem_code_page(775, &[0xA1]), "Ī"); // Baltic
        assert_eq!(decode_with_oem_code_page(852, &[0xA5]), "ą"); // Central Europe
        assert_eq!(decode_with_oem_code_page(855, &[0x80]), "ђ"); // Cyrillic IBM
        assert_eq!(decode_with_oem_code_page(858, &[0xD5]), "€"); // 850 + euro
        assert_eq!(decode_with_oem_code_page(869, &[0x8F]), "Ή"); // Greek modern
                                                                  // ASCII is invariant across the single-byte DOS pages.
        assert_eq!(decode_with_oem_code_page(862, b"abc"), "abc");
    }
}

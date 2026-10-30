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
            Err(err) => return ExecResult::StartFailed(err.to_string()),
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
    (String::from_utf8_lossy(&kept).into_owned(), truncated)
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
        // process_group(0) made the child a group leader; `kill -9 -<pgid>`
        // (negative target = process group, per procps kill(1)) takes the
        // whole tree down without orphaned children.
        let _ = Command::new("kill")
            .args(["-9", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Belt and braces: if the group is already gone this is a no-op.
        let _ = Command::new("kill")
            .args(["-9", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
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
    pub fn from_argv(command: Vec<String>, timeout: Duration) -> Self {
        Self {
            command,
            timeout,
            max_output_bytes: 64 * 1024,
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
        assert!(
            start.elapsed() < Duration::from_secs(2),
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
}

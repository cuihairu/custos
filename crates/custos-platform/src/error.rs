//! The unified platform failure model (todo.md P2: 平台特有错误 → 统一故障
//! 结果转换). Every adapter maps its platform-specific failure onto these
//! variants so the engine and toolbox never see raw OS or command output.

use custos_probe::exec::ExecResult;

/// One platform operation failure. Carries no secrets; detail strings are
/// scrubbed output snippets or the platform's own message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    /// The platform cannot serve this operation at all (wrong init system,
    /// missing tool, wrong OS) — surface the reason, never guess-execute.
    #[error("unsupported on this platform: {0}")]
    Unsupported(String),
    /// The named target (unit, interface, process) does not exist.
    #[error("{0} not found")]
    NotFound(String),
    /// The caller is not authorized (the toolbox permission gate refused).
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// The operation exceeded its deadline.
    #[error("timed out: {0}")]
    TimedOut(String),
    /// The platform reported a busy/conflict state (e.g. already stopping).
    #[error("busy: {0}")]
    Busy(String),
    /// The operation ran and failed; detail carries the platform's answer.
    #[error("failed: {0}")]
    Failed(String),
}

/// Convenience alias for adapter results.
pub type PlatformResult<T> = Result<T, PlatformError>;

/// Maps one controlled-execution result (the shared hardened runner) onto
/// the unified failure model.
impl From<ExecResult> for PlatformError {
    fn from(result: ExecResult) -> Self {
        match result {
            ExecResult::TimedOut => PlatformError::TimedOut("command exceeded its deadline".into()),
            ExecResult::StartFailed(detail) => {
                // A missing tool is Unsupported (guess-executing a different
                // tool would fake support), anything else is Failed.
                PlatformError::Unsupported(format!("tool not runnable: {detail}"))
            }
            ExecResult::Failed {
                code,
                stdout,
                stderr,
                ..
            } => {
                let detail = pick_detail(&stdout, &stderr);
                match code {
                    // systemctl: unit not loaded → 4/5; sc: service not
                    // found → 1060-ish text. Normalize the common "missing
                    // target" exits; the message still carries the code.
                    4 | 5 if !detail.is_empty() => {
                        PlatformError::NotFound(format!("exit {code}: {detail}"))
                    }
                    _ => PlatformError::Failed(format!("exit {code}: {detail}")),
                }
            }
            ExecResult::Succeeded { .. } => {
                unreachable!("a successful execution is not an error")
            }
        }
    }
}

/// Picks the most informative output snippet (stderr first, capped).
fn pick_detail(stdout: &str, stderr: &str) -> String {
    let chosen = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let trimmed = chosen.trim();
    if trimmed.chars().count() > 200 {
        trimmed.chars().take(200).collect::<String>() + "…"
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_failures_map_onto_unified_errors() {
        let err = PlatformError::from(ExecResult::Failed {
            code: 1,
            stdout: String::new(),
            stderr: "Unit nginx.service not loaded.".into(),
            truncated: false,
        });
        assert!(matches!(err, PlatformError::Failed(_)));
        assert!(err.to_string().contains("nginx"), "{err}");

        // systemctl exit 4 with a message reads as NotFound.
        let err = PlatformError::from(ExecResult::Failed {
            code: 4,
            stdout: String::new(),
            stderr: "Unit ghost.service not loaded.".into(),
            truncated: false,
        });
        assert!(matches!(err, PlatformError::NotFound(_)), "{err}");
    }

    #[test]
    fn timeout_and_missing_tool_are_distinct() {
        assert!(matches!(
            PlatformError::from(ExecResult::TimedOut),
            PlatformError::TimedOut(_)
        ));
        let err = PlatformError::from(ExecResult::StartFailed(
            "No such file or directory (os error 2)".into(),
        ));
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
    }

    #[test]
    fn long_output_is_capped() {
        let err = PlatformError::from(ExecResult::Failed {
            code: 1,
            stdout: "x".repeat(5000),
            stderr: String::new(),
            truncated: false,
        });
        let detail = match err {
            PlatformError::Failed(d) => d,
            other => panic!("{other:?}"),
        };
        assert!(detail.chars().count() < 220, "{}", detail.chars().count());
    }
}

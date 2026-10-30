//! The controlled-command-execution builtin (ADR-0015): one hardened
//! implementation (timeout, output cap, process-tree cleanup —
//! `custos_probe::exec::run_controlled`) that every future caller shares,
//! instead of each plugin rolling its own subprocess handling.

use crate::registry::{BuiltinTool, ToolOutcome, ToolboxError, ToolboxErrorKind};
use custos_plugin_api::{AffectedScope, Capability, CapabilityKind, OperationParams};

/// Default deadline for a controlled execution.
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// Default output cap, matching the exec probe.
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// `exec.run`: executes an explicit argv under a deadline.
pub struct ExecTool;

impl ExecTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExecTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinTool for ExecTool {
    fn capability(&self) -> Capability {
        Capability {
            kind: CapabilityKind::RecoveryActionProvider,
            operation_id: "exec.run".into(),
            summary: "controlled command execution with timeout and output caps".into(),
            read_only: false,
            required_permission: "host.exec".into(),
            affected_scope: AffectedScope::NamedResource,
            interrupts_network: false,
            interrupts_remote_admin: false,
            cancelable: false,
            idempotent: false,
            verifiable: true,
            possible_failures: vec![
                "nonzero_exit".into(),
                "timed_out".into(),
                "start_failed".into(),
            ],
            compensation_operation: None,
        }
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        use custos_probe::exec::ExecResult;
        let argv = params
            .args
            .get("argv")
            .and_then(|v| v.as_array())
            .filter(|arr| !arr.is_empty())
            .and_then(|arr| {
                arr.iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<Vec<String>>>()
            })
            .ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`argv` must be a non-empty array of strings",
                )
            })?;
        // §4.3: parameters are data, never shell text — argv is passed
        // through directly, no shell, no concatenation.
        let timeout_ms = match params.args.get("timeout_ms") {
            None => DEFAULT_TIMEOUT_MS,
            Some(v) => v.as_u64().filter(|ms| *ms > 0).ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`timeout_ms` must be a positive integer",
                )
            })?,
        };
        let max_output = match params.args.get("max_output_bytes") {
            None => DEFAULT_MAX_OUTPUT_BYTES,
            Some(v) => v.as_u64().filter(|n| *n > 0).ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`max_output_bytes` must be a positive integer",
                )
            })? as usize,
        };
        match custos_probe::exec::run_controlled(
            &argv,
            std::time::Duration::from_millis(timeout_ms),
            max_output,
        ) {
            ExecResult::Succeeded { stdout, truncated } => Ok(ToolOutcome::Action {
                succeeded: true,
                detail: describe_output(&stdout, truncated),
            }),
            ExecResult::Failed {
                code,
                stdout,
                stderr,
                truncated,
            } => Ok(ToolOutcome::Action {
                succeeded: false,
                detail: format!(
                    "exit {code}; stdout: {}; stderr: {}",
                    describe_output(&stdout, truncated),
                    describe_output(&stderr, truncated)
                ),
            }),
            ExecResult::TimedOut => Err(ToolboxError::new(
                ToolboxErrorKind::TimedOut,
                format!("command exceeded {} ms", timeout_ms),
            )),
            ExecResult::StartFailed(err) => Err(ToolboxError::new(
                ToolboxErrorKind::Failed,
                format!("could not start: {err}"),
            )),
        }
    }
}

fn describe_output(output: &str, truncated: bool) -> String {
    if truncated {
        format!("{output}… [truncated]")
    } else {
        output.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn params(args: BTreeMap<String, serde_json::Value>) -> OperationParams {
        OperationParams {
            operation_id: "exec.run".into(),
            resource_id: "r".into(),
            run_id: None,
            args,
        }
    }

    #[test]
    fn capability_declares_exec_permission_and_dangers() {
        let cap = ExecTool::new().capability();
        assert!(!cap.read_only);
        assert_eq!(cap.required_permission, "host.exec");
        assert!(cap.possible_failures.contains(&"timed_out".to_owned()));
    }

    #[test]
    fn runs_explicit_argv_and_reports_success() {
        let outcome = ExecTool::new()
            .execute(&params(BTreeMap::from([
                (
                    "argv".into(),
                    serde_json::json!(["sh", "-c", "echo controlled-exec-ok"]),
                ),
                ("timeout_ms".into(), serde_json::json!(2000)),
            ])))
            .unwrap();
        match outcome {
            ToolOutcome::Action { succeeded, detail } => {
                assert!(succeeded);
                assert!(detail.contains("controlled-exec-ok"), "{detail}");
            }
            other => panic!("expected action outcome, got {other:?}"),
        }
    }

    #[test]
    fn nonzero_exit_is_a_failed_outcome_not_an_error() {
        let outcome = ExecTool::new()
            .execute(&params(BTreeMap::from([(
                "argv".into(),
                serde_json::json!(["sh", "-c", "exit 3"]),
            )])))
            .unwrap();
        match outcome {
            ToolOutcome::Action { succeeded, detail } => {
                assert!(!succeeded);
                assert!(detail.contains("exit 3"), "{detail}");
            }
            other => panic!("expected action outcome, got {other:?}"),
        }
    }

    #[test]
    fn timeouts_are_deadline_errors() {
        let err = ExecTool::new()
            .execute(&params(BTreeMap::from([
                ("argv".into(), serde_json::json!(["sh", "-c", "sleep 5"])),
                ("timeout_ms".into(), serde_json::json!(200)),
            ])))
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::TimedOut);
    }

    #[test]
    fn argv_must_be_a_non_empty_string_array() {
        for bad in [
            serde_json::json!([]),
            serde_json::json!(["sh", 42]),
            serde_json::json!("sh -c 'echo hi'"),
        ] {
            let err = ExecTool::new()
                .execute(&params(BTreeMap::from([("argv".into(), bad)])))
                .unwrap_err();
            assert_eq!(err.kind, ToolboxErrorKind::InvalidParams, "{err}");
        }
    }
}

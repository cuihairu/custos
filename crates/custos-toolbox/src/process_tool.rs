//! The process-status builtin (ADR-0015): process liveness probing as a
//! read-only capability, reusing the hardened `ProcessProbe` (pid file +
//! liveness check) rather than re-implementing platform process checks.

use crate::registry::{BuiltinTool, ToolOutcome, ToolboxError, ToolboxErrorKind};
use custos_model::{HealthStatus, Resource};
use custos_plugin_api::{AffectedScope, Capability, CapabilityKind, OperationParams};
use custos_probe::process::ProcessProbe;
use custos_probe::Probe;

/// `process.status`: checks whether the process behind a pid file is alive.
pub struct ProcessStatusTool;

impl ProcessStatusTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProcessStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinTool for ProcessStatusTool {
    fn capability(&self) -> Capability {
        Capability {
            kind: CapabilityKind::ProbeProvider,
            operation_id: "process.status".into(),
            summary: "process liveness check behind a pid file".into(),
            read_only: true,
            required_permission: "process.read".into(),
            affected_scope: AffectedScope::NamedResource,
            interrupts_network: false,
            interrupts_remote_admin: false,
            cancelable: false,
            idempotent: true,
            verifiable: true,
            possible_failures: vec!["pid_file_missing".into()],
            compensation_operation: None,
        }
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        let pid_file = params
            .args
            .get("pid_file")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`pid_file` must be a non-empty path string",
                )
            })?;
        let mut resource = Resource::new(params.resource_id.as_str(), "process");
        resource.config = serde_json::json!({ "pid_file": pid_file });
        let report = ProcessProbe {
            pid_file: pid_file.into(),
            rss_threshold_kb: None,
        }
        .probe(&resource);
        let healthy = report.status == HealthStatus::Healthy;
        Ok(ToolOutcome::Probe {
            healthy,
            observation: report.observation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn params(pid_file: &str) -> OperationParams {
        OperationParams {
            operation_id: "process.status".into(),
            resource_id: "worker".into(),
            run_id: None,
            args: BTreeMap::from([(
                "pid_file".into(),
                serde_json::Value::String(pid_file.into()),
            )]),
        }
    }

    #[test]
    fn capability_is_read_only() {
        let cap = ProcessStatusTool::new().capability();
        assert!(cap.read_only);
        assert_eq!(cap.required_permission, "process.read");
    }

    #[test]
    fn missing_params_are_invalid() {
        let err = ProcessStatusTool::new()
            .execute(&OperationParams {
                operation_id: "process.status".into(),
                resource_id: "w".into(),
                run_id: None,
                args: Default::default(),
            })
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::InvalidParams);
    }

    #[test]
    fn dead_pid_reports_unhealthy_observation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.pid");
        std::fs::write(&path, "99999999\n").unwrap(); // a pid that cannot exist
        let outcome = ProcessStatusTool::new()
            .execute(&params(path.to_str().unwrap()))
            .unwrap();
        match outcome {
            ToolOutcome::Probe {
                healthy,
                observation,
            } => {
                assert!(!healthy, "{observation}");
                assert!(observation.contains("99999999"), "{observation}");
            }
            other => panic!("expected probe outcome, got {other:?}"),
        }
    }

    #[test]
    fn live_pid_reports_healthy() {
        let pid = std::process::id();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.pid");
        std::fs::write(&path, format!("{pid}\n")).unwrap();
        let outcome = ProcessStatusTool::new()
            .execute(&params(path.to_str().unwrap()))
            .unwrap();
        assert!(matches!(outcome, ToolOutcome::Probe { healthy: true, .. }));
    }
}

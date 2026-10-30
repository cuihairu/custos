//! Capability declarations (docs/guide/plugins.md §4.6) and parameterized
//! operation inputs (§4.3: explicit operation ids, never shell text).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A capability kind a plugin can provide (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    ResourceProvider,
    ProbeProvider,
    DiagnosisProvider,
    RecoveryActionProvider,
    VerificationProvider,
    NotificationChannelProvider,
}

impl CapabilityKind {
    /// Stable snake_case name, identical to the wire form (§4.6) — used in
    /// logs and compatibility reports.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ResourceProvider => "resource_provider",
            Self::ProbeProvider => "probe_provider",
            Self::DiagnosisProvider => "diagnosis_provider",
            Self::RecoveryActionProvider => "recovery_action_provider",
            Self::VerificationProvider => "verification_provider",
            Self::NotificationChannelProvider => "notification_channel_provider",
        }
    }
}

/// One declared capability with the eleven mandatory metadata fields of
/// §4.6. The host decides from these (plus configuration policy) whether an
/// action may run — a declaration alone grants nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub kind: CapabilityKind,
    /// Stable operation id used in `execute_recovery_action` requests.
    pub operation_id: String,
    pub summary: String,
    /// Read-only (`true`) or state-modifying (`false`).
    pub read_only: bool,
    /// Permission class required to execute (e.g. "host.restart").
    pub required_permission: String,
    /// Scope of resources this capability can affect.
    pub affected_scope: AffectedScope,
    /// Execution can tear down network connectivity.
    pub interrupts_network: bool,
    /// Execution can tear down the remote management channel.
    pub interrupts_remote_admin: bool,
    /// Supports cooperative cancellation.
    pub cancelable: bool,
    /// Repeated execution is safe.
    pub idempotent: bool,
    /// Health can be independently verified afterwards.
    pub verifiable: bool,
    /// Failure states the host must be prepared to handle.
    pub possible_failures: Vec<String>,
    /// Compensating operation id, if one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation_operation: Option<String>,
}

/// Which resources a capability touches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AffectedScope {
    /// Only the resource the request names.
    NamedResource,
    /// Multiple resources listed per call.
    ListedResources,
    /// The whole host (reboot, network stack resets).
    Host,
}

/// Parameterized operation input (§4.3): named scalars/arrays passed as
/// data. Values are never interpreted as shell text; hosts must pass them
/// as argv/environment/JSON to the underlying mechanism, never concatenate
/// them into a command line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationParams {
    pub operation_id: String,
    /// Resource the operation targets.
    pub resource_id: String,
    /// Recovery run correlation, when inside one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Named parameters; values are JSON scalars or arrays of scalars.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability() -> Capability {
        Capability {
            kind: CapabilityKind::RecoveryActionProvider,
            operation_id: "service.restart".into(),
            summary: "restart a systemd unit".into(),
            read_only: false,
            required_permission: "service.manage".into(),
            affected_scope: AffectedScope::NamedResource,
            interrupts_network: false,
            interrupts_remote_admin: false,
            cancelable: true,
            idempotent: true,
            verifiable: true,
            possible_failures: vec!["unit_missing".into(), "permission_denied".into()],
            compensation_operation: Some("service.stop".into()),
        }
    }

    #[test]
    fn capability_roundtrips_with_all_eleven_fields() {
        let cap = capability();
        let json = serde_json::to_string(&cap).unwrap();
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cap);
        for field in [
            "\"kind\"",
            "\"operation_id\"",
            "\"read_only\"",
            "\"required_permission\"",
            "\"affected_scope\"",
            "\"interrupts_network\"",
            "\"interrupts_remote_admin\"",
            "\"cancelable\"",
            "\"idempotent\"",
            "\"verifiable\"",
            "\"possible_failures\"",
            "\"compensation_operation\"",
        ] {
            assert!(json.contains(field), "missing {field} in {json}");
        }
    }

    #[test]
    fn operation_params_are_structured_not_shell_text() {
        let params = OperationParams {
            operation_id: "service.restart".into(),
            resource_id: "openclash".into(),
            run_id: Some("run-42".into()),
            args: BTreeMap::from([("unit".into(), serde_json::Value::String("openclash".into()))]),
        };
        let json = serde_json::to_string(&params).unwrap();
        let back: OperationParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, params);
        // No positional command line anywhere: arguments are named data.
        assert!(json.contains("\"args\""));
    }
}

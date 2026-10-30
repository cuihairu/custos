//! Capability compatibility validation for the plugin protocol.

use crate::session::HostError;
use custos_plugin_api::capability::Capability;
use std::collections::HashSet;

/// Checks the capability set the plugin declared during negotiation.
///
/// A plugin is rejected when it declares no capabilities at all, or when an
/// operation id appears twice — under the same kind or across kinds. The
/// host dispatches by operation id alone, so any collision is ambiguous.
pub fn validate_capabilities(
    caps: &[Capability],
    required: &[CapabilityKindRequirement],
) -> Result<(), HostError> {
    if caps.is_empty() {
        return Err(HostError::NoCapabilities);
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for cap in caps {
        if !seen.insert(cap.operation_id.as_str()) {
            return Err(HostError::DuplicateCapability {
                kind: cap.kind.name().to_owned(),
                operation_id: cap.operation_id.clone(),
            });
        }
    }
    for req in required {
        if !caps
            .iter()
            .any(|c| c.kind == req.kind && c.operation_id == req.operation_id)
        {
            return Err(HostError::MissingRequiredCapability {
                kind: req.kind.name().to_owned(),
                operation_id: req.operation_id.clone(),
            });
        }
    }
    Ok(())
}

/// A capability the host requires the plugin to declare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityKindRequirement {
    pub kind: custos_plugin_api::CapabilityKind,
    pub operation_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_plugin_api::{AffectedScope, Capability, CapabilityKind, OperationParams};

    fn cap(kind: CapabilityKind, op: &str) -> Capability {
        Capability {
            kind,
            operation_id: op.into(),
            summary: "s".into(),
            read_only: kind == CapabilityKind::ProbeProvider,
            required_permission: "perm".into(),
            affected_scope: AffectedScope::NamedResource,
            interrupts_network: false,
            interrupts_remote_admin: false,
            cancelable: false,
            idempotent: false,
            verifiable: false,
            possible_failures: vec![],
            compensation_operation: None,
        }
    }

    #[test]
    fn empty_capability_set_is_rejected() {
        let err = validate_capabilities(&[], &[]).unwrap_err();
        assert!(matches!(err, HostError::NoCapabilities));
    }

    #[test]
    fn duplicate_operation_id_for_same_kind_is_rejected() {
        let caps = vec![
            cap(CapabilityKind::RecoveryActionProvider, "svc.restart"),
            cap(CapabilityKind::RecoveryActionProvider, "svc.restart"),
        ];
        let err = validate_capabilities(&caps, &[]).unwrap_err();
        assert!(matches!(err, HostError::DuplicateCapability { .. }));
    }

    #[test]
    fn operation_id_reused_across_kinds_is_rejected() {
        let caps = vec![
            cap(CapabilityKind::ProbeProvider, "svc.status"),
            cap(CapabilityKind::RecoveryActionProvider, "svc.status"),
        ];
        let err = validate_capabilities(&caps, &[]).unwrap_err();
        assert!(matches!(err, HostError::DuplicateCapability { .. }));
    }

    #[test]
    fn missing_required_capability_is_rejected() {
        let caps = vec![cap(CapabilityKind::ProbeProvider, "svc.status")];
        let req = CapabilityKindRequirement {
            kind: CapabilityKind::RecoveryActionProvider,
            operation_id: "svc.restart".into(),
        };
        let err = validate_capabilities(&caps, &[req]).unwrap_err();
        assert!(matches!(err, HostError::MissingRequiredCapability { .. }));
    }

    #[test]
    fn a_valid_capability_set_passes() {
        let caps = vec![
            cap(CapabilityKind::ProbeProvider, "svc.status"),
            cap(CapabilityKind::RecoveryActionProvider, "svc.restart"),
        ];
        validate_capabilities(&caps, &[]).unwrap();
        let req = CapabilityKindRequirement {
            kind: CapabilityKind::RecoveryActionProvider,
            operation_id: "svc.restart".into(),
        };
        validate_capabilities(&caps, &[req]).unwrap();
    }

    #[test]
    fn operation_params_are_parameterized() {
        let params = OperationParams {
            operation_id: "svc.restart".into(),
            resource_id: "svc".into(),
            run_id: None,
            args: Default::default(),
        };
        assert_eq!(params.operation_id, "svc.restart");
    }
}

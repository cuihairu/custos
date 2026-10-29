//! Traceable audit events (docs/guide/persistence.md §7.2 `audit_events`).

use crate::ids::ResourceId;
use serde::{Deserialize, Serialize};

/// A traceable system event. Every state transition and action attempt must
/// be auditable (docs/guide/recovery-engine.md, agent protocol §11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_type: String,
    pub resource_id: Option<ResourceId>,
    pub summary: String,
    pub occurred_at_unix_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        let event = AuditEvent {
            event_type: "recovery.action_started".into(),
            resource_id: Some(ResourceId::new("proxy.egress")),
            summary: "switch node".into(),
            occurred_at_unix_ms: 1_790_000_000_000,
        };
        let back: AuditEvent =
            serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
        assert_eq!(back, event);
    }
}

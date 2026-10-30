//! Incident lifecycle (docs/guide/persistence.md §7.2 `incidents`).

use crate::diagnosis::FaultCode;
use crate::ids::{IncidentId, ResourceId};
use serde::{Deserialize, Serialize};

/// One fault lifecycle: opened when a failure is confirmed, closed when the
/// resource is verified healthy again or handling stops. Open incidents are
/// active state — retention purging must never remove them (§7.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    pub id: IncidentId,
    pub resource_id: ResourceId,
    pub fault_code: FaultCode,
    pub summary: String,
    pub opened_at_unix_ms: u64,
    /// Set when the incident closed (recovered, suppressed or escalated to
    /// the human). `None` = still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at_unix_ms: Option<u64>,
}

impl Incident {
    pub fn open(
        id: IncidentId,
        resource_id: ResourceId,
        fault_code: FaultCode,
        summary: impl Into<String>,
        at_unix_ms: u64,
    ) -> Self {
        Self {
            id,
            resource_id,
            fault_code,
            summary: summary.into(),
            opened_at_unix_ms: at_unix_ms,
            closed_at_unix_ms: None,
        }
    }

    pub fn close(&mut self, at_unix_ms: u64) {
        self.closed_at_unix_ms = Some(at_unix_ms);
    }

    pub fn is_open(&self) -> bool {
        self.closed_at_unix_ms.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_then_close_roundtrip() {
        let mut incident = Incident::open(
            IncidentId::new("inc-1"),
            ResourceId::new("db"),
            FaultCode::ServiceStopped,
            "db unreachable",
            1_000,
        );
        assert!(incident.is_open());
        incident.close(2_000);
        assert!(!incident.is_open());
        assert_eq!(incident.closed_at_unix_ms, Some(2_000));

        let json = serde_json::to_string(&incident).unwrap();
        let back: Incident = serde_json::from_str(&json).unwrap();
        assert_eq!(back, incident);
    }
}

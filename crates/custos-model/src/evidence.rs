//! Probe evidence (docs/guide/overview.md §2.1: Evidence = probe results,
//! errors, timestamps and related evidence).

use crate::ids::ResourceId;
use serde::{Deserialize, Serialize};

/// One piece of evidence collected by a probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub resource_id: ResourceId,
    pub probe_name: String,
    pub succeeded: bool,
    /// Human-readable observation, e.g. "default gateway reachable".
    pub observation: String,
    pub collected_at_unix_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        let evidence = Evidence {
            resource_id: crate::ids::ResourceId::new("wifi"),
            probe_name: "connectivity".into(),
            succeeded: false,
            observation: "public IP unreachable".into(),
            collected_at_unix_ms: 1_790_000_000_000,
        };
        let json = serde_json::to_string(&evidence).unwrap();
        let back: Evidence = serde_json::from_str(&json).unwrap();
        assert_eq!(back, evidence);
    }
}

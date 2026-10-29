//! Resource definitions (docs/guide/resources.md §3.1, §3.2).

use crate::dependency::Dependency;
use crate::ids::ResourceId;
use serde::{Deserialize, Serialize};

/// Health decision policy for one resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthPolicy {
    /// Consecutive failures required to enter a fault state.
    pub failure_threshold: u32,
    /// Consecutive successes required to leave it.
    pub success_threshold: u32,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        // docs/guide/probe-diagnosis.md §5.3: example values; real values are
        // set per resource type.
        Self {
            failure_threshold: 3,
            success_threshold: 2,
        }
    }
}

/// Recovery constraints for one resource. Candidate plans are added in P0
/// engine work; the model only carries the limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecoveryPolicy {
    /// Maximum recovery attempts for this resource.
    pub max_attempts: Option<u32>,
    /// High-risk actions require explicit configuration opt-in
    /// (docs/guide/security.md §12.2).
    pub allow_high_risk: bool,
}

/// The basic object Custos monitors and recovers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub id: ResourceId,
    /// Resource type, e.g. `process`, `proxy`, `tunnel`.
    pub resource_type: String,
    pub display_name: String,
    /// Plugin implementing this resource's capabilities.
    pub plugin_ref: Option<String>,
    /// Resource-type-specific configuration.
    pub config: serde_json::Value,
    pub health_policy: HealthPolicy,
    pub recovery_policy: RecoveryPolicy,
    /// Upstream dependencies of this resource.
    pub dependencies: Vec<Dependency>,
    pub enabled: bool,
}

impl Resource {
    pub fn new(id: impl Into<ResourceId>, resource_type: &str) -> Self {
        Self {
            id: id.into(),
            resource_type: resource_type.to_owned(),
            display_name: String::new(),
            plugin_ref: None,
            config: serde_json::Value::Null,
            health_policy: HealthPolicy::default(),
            recovery_policy: RecoveryPolicy::default(),
            dependencies: Vec::new(),
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_defaults_are_enabled_with_example_thresholds() {
        let resource = Resource::new("proxy.egress", "proxy");
        assert!(resource.enabled);
        assert_eq!(resource.health_policy.failure_threshold, 3);
    }

    #[test]
    fn resource_roundtrips_with_dependencies() {
        let mut resource = Resource::new("tunnel", "tunnel");
        resource
            .dependencies
            .push(Dependency::required(ResourceId::new("proxy.egress")));
        let back: Resource =
            serde_json::from_str(&serde_json::to_string(&resource).unwrap()).unwrap();
        assert_eq!(back, resource);
    }
}

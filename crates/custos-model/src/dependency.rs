//! Dependency relationships between resources (docs/guide/resources.md §3.3,
//! §3.4).

use crate::health::HealthStatus;
use crate::ids::ResourceId;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How a dependency constrains upstream actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyRequirement {
    /// Must be satisfied; blocks dependent recovery actions.
    Required,
    /// Recovery may be attempted; blocking is decided by policy.
    Optional,
    /// Only required under specific conditions.
    Conditional,
    /// A set of candidates where the specified number must be satisfied.
    AlternativeGroup,
}

/// The health condition a dependency declares. A dependency must declare
/// which condition it depends on, not merely which resource (§3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCondition {
    /// Resource status must equal the given state.
    StatusIs(HealthStatus),
    /// Last successful probe no older than the given seconds.
    ProbedWithin { seconds: u64 },
}

/// One dependency edge: `dependent -> target`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub target_resource_id: ResourceId,
    pub requirement: DependencyRequirement,
    pub condition: HealthCondition,
    #[serde(skip)]
    pub timeout: Option<Duration>,
}

impl Dependency {
    pub fn required(target: impl Into<ResourceId>) -> Self {
        Self {
            target_resource_id: target.into(),
            requirement: DependencyRequirement::Required,
            condition: HealthCondition::StatusIs(HealthStatus::Healthy),
            timeout: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_requirement_and_condition() {
        let dep = Dependency::required(ResourceId::new("network"));
        let value = serde_json::to_value(&dep).unwrap();
        assert_eq!(value["requirement"], "required");
        assert_eq!(value["condition"]["status_is"], "healthy");
    }
}

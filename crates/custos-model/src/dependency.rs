//! Dependency relationships between resources (docs/guide/resources.md §3.3,
//! §3.4).

use crate::health::HealthStatus;
use crate::ids::ResourceId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

/// How a dependency constrains upstream actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyRequirement {
    /// Must be satisfied; blocks dependent recovery actions.
    Required,
    /// Recovery may be attempted; blocking is decided by policy.
    Optional,
    /// Only required when the mode named by `Dependency::conditional_on` is
    /// active (e.g. proxy mode vs. direct mode).
    Conditional,
    /// Satisfied through an [`AlternativeGroup`] instead of a single target.
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
    /// A named capability must be available.
    CapabilityAvailable { capability: String },
    /// Consecutive successes must reach the given count.
    ConsecutiveSuccesses { count: u32 },
    /// A named business check must pass.
    BusinessCheck { check: String },
}

impl HealthCondition {
    /// Conditions that can never be satisfied are configuration errors
    /// (docs/guide/resources.md §3.5 item 5).
    pub fn is_impossible(&self) -> bool {
        match self {
            Self::ProbedWithin { seconds } => *seconds == 0,
            Self::ConsecutiveSuccesses { count } => *count == 0,
            _ => false,
        }
    }
}

/// One dependency edge: `dependent -> target`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub target_resource_id: ResourceId,
    pub requirement: DependencyRequirement,
    pub condition: HealthCondition,
    /// Mode key that activates a `Conditional` dependency; ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditional_on: Option<String>,
    #[serde(skip)]
    pub timeout: Option<Duration>,
}

impl Dependency {
    pub fn required(target: impl Into<ResourceId>) -> Self {
        Self {
            target_resource_id: target.into(),
            requirement: DependencyRequirement::Required,
            condition: HealthCondition::StatusIs(HealthStatus::Healthy),
            conditional_on: None,
            timeout: None,
        }
    }

    /// Whether this dependency constrains the current run, given the active
    /// mode keys. Optional and inactive Conditional dependencies do not
    /// block upstream actions.
    pub fn is_binding(&self, active_modes: &HashSet<String>) -> bool {
        match self.requirement {
            DependencyRequirement::Conditional => self
                .conditional_on
                .as_ref()
                .is_some_and(|mode| active_modes.contains(mode)),
            DependencyRequirement::Optional => false,
            DependencyRequirement::Required | DependencyRequirement::AlternativeGroup => true,
        }
    }
}

/// N-of-M backup semantics (docs/guide/resources.md §3.3 AlternativeGroup):
/// backup resources must not be modeled as "every dependency must be
/// healthy".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlternativeGroup {
    pub group_id: String,
    pub members: Vec<ResourceId>,
    pub required_count: usize,
}

impl AlternativeGroup {
    /// True when at least `required_count` members satisfy `healthy`.
    pub fn satisfied_by(&self, healthy: &impl Fn(&ResourceId) -> bool) -> bool {
        self.members.iter().filter(|m| healthy(m)).count() >= self.required_count
    }

    /// A group requiring more members than it has can never be satisfied
    /// (docs/guide/resources.md §3.5 item 5).
    pub fn is_impossible(&self) -> bool {
        self.required_count > self.members.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn serializes_requirement_and_condition() {
        let dep = Dependency::required(ResourceId::new("network"));
        let value = serde_json::to_value(&dep).unwrap();
        assert_eq!(value["requirement"], "required");
        assert_eq!(value["condition"]["status_is"], "healthy");
    }

    #[test]
    fn conditional_dependency_binds_only_in_mode() {
        let mut dep = Dependency::required(ResourceId::new("proxy"));
        dep.requirement = DependencyRequirement::Conditional;
        dep.conditional_on = Some("proxy_mode".into());

        assert!(!dep.is_binding(&modes(&[])));
        assert!(dep.is_binding(&modes(&["proxy_mode"])));
    }

    #[test]
    fn optional_dependency_never_binds() {
        let mut dep = Dependency::required(ResourceId::new("proxy"));
        dep.requirement = DependencyRequirement::Optional;
        assert!(!dep.is_binding(&modes(&[])));
    }

    #[test]
    fn alternative_group_satisfied_by_member_count() {
        let group = AlternativeGroup {
            group_id: "egress".into(),
            members: vec![ResourceId::new("proxy-a"), ResourceId::new("proxy-b")],
            required_count: 1,
        };
        assert!(group.satisfied_by(&|id| id.as_str() == "proxy-a"));
        assert!(!group.satisfied_by(&|_| false));
    }

    #[test]
    fn impossible_conditions_are_detected() {
        assert!(HealthCondition::ProbedWithin { seconds: 0 }.is_impossible());
        assert!(HealthCondition::ConsecutiveSuccesses { count: 0 }.is_impossible());
        assert!(!HealthCondition::ProbedWithin { seconds: 5 }.is_impossible());

        let group = AlternativeGroup {
            group_id: "egress".into(),
            members: vec![ResourceId::new("proxy-a")],
            required_count: 2,
        };
        assert!(group.is_impossible());
    }
}

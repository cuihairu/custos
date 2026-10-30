//! Configuration parsing and validation (docs/guide/dashboard-cli-config.md
//! §10, docs/guide/resources.md §3.5).
//!
//! Configuration load time is the gate: unknown fields are errors, duplicate
//! IDs are errors, dangling dependency references are errors, impossible
//! dependency conditions are errors, and dependency cycles are errors —
//! startup is rejected instead of recursing forever during recovery.
//! Every validation failure reports the concrete field path (§10.3).

use custos_model::{
    ActionSpec, AlternativeGroup, Dependency, DependencyGraph, FaultCode, GraphError,
    HealthCondition, HealthPolicy, RecoveryPlan, RecoveryPolicy, Resource, ResourceId,
};
use serde::Deserialize;

/// Supported configuration schema version.
pub const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported configuration version {found} at `version` (supported: {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("{path}: {message}")]
    Validation { path: String, message: String },
}

impl ConfigError {
    fn validation(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Probe wiring as written per resource; carried into `Resource::config`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProbe {
    /// `custom-command`: check command argv.
    #[serde(default)]
    command: Option<Vec<String>>,
    /// `process`: file containing the PID to watch.
    #[serde(default)]
    pid_file: Option<String>,
    #[serde(default = "default_interval_ms")]
    interval_ms: u64,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    max_output_bytes: usize,
}

fn default_interval_ms() -> u64 {
    10_000
}
fn default_timeout_ms() -> u64 {
    3_000
}
fn default_max_output_bytes() -> usize {
    64 * 1024
}

/// One dependency edge as written in the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDependency {
    resource: String,
    #[serde(default)]
    requirement: Option<String>,
    #[serde(default)]
    condition: Option<HealthCondition>,
    #[serde(default)]
    conditional_on: Option<String>,
}

/// One candidate recovery plan as written in the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    plan_id: String,
    applicable_faults: Vec<FaultCode>,
    #[serde(default)]
    risk: Option<custos_model::RiskLevel>,
    action: ActionSpec,
    #[serde(default = "default_plan_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_plan_attempts")]
    max_attempts: u32,
    #[serde(default = "default_plan_cooldown_ms")]
    cooldown_ms: u64,
    #[serde(default)]
    idempotent: bool,
    #[serde(default)]
    verification: Option<HealthCondition>,
    #[serde(default)]
    failure_handling: Option<custos_model::FailureHandling>,
    #[serde(default)]
    compensation: Option<String>,
}

fn default_plan_timeout_ms() -> u64 {
    10_000
}
fn default_plan_attempts() -> u32 {
    1
}
fn default_plan_cooldown_ms() -> u64 {
    30_000
}

/// One resource entry as written in the configuration file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResource {
    id: String,
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    probe: Option<RawProbe>,
    #[serde(default)]
    health_policy: Option<PartialHealthPolicy>,
    #[serde(default)]
    recovery_policy: Option<RawRecoveryPolicy>,
    /// Shorthand for `dependencies` with `required` requirement.
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    dependencies: Vec<RawDependency>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialHealthPolicy {
    #[serde(default = "default_failure_threshold")]
    failure_threshold: u32,
    #[serde(default = "default_success_threshold")]
    success_threshold: u32,
}

fn default_failure_threshold() -> u32 {
    3
}
fn default_success_threshold() -> u32 {
    2
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecoveryPolicy {
    #[serde(default)]
    plans: Vec<RawPlan>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    allow_high_risk: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAlternativeGroup {
    id: String,
    members: Vec<String>,
    #[serde(default = "default_required_count")]
    required: usize,
}

fn default_required_count() -> usize {
    1
}

/// Top-level configuration document. Unknown fields must not be silently
/// interpreted as valid configuration (docs/guide/dashboard-cli-config.md §10.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    version: u32,
    #[serde(default)]
    resources: Vec<RawResource>,
    #[serde(default)]
    alternative_groups: Vec<RawAlternativeGroup>,
}

/// Validated configuration ready for the runtime registry.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedConfig {
    pub resources: Vec<Resource>,
    pub alternative_groups: Vec<AlternativeGroup>,
    pub graph: DependencyGraph,
}

impl ValidatedConfig {
    pub fn resource(&self, id: &ResourceId) -> Option<&Resource> {
        self.resources.iter().find(|r| &r.id == id)
    }
}

/// Parses, validates and builds the dependency graph. This is the startup
/// path: any error here must abort startup (docs/guide/resources.md §3.5 —
/// "reject at configuration stage, never recurse during recovery").
pub fn load(config_text: &str) -> Result<ValidatedConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(config_text)?;

    if raw.version != SUPPORTED_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            found: raw.version,
            supported: SUPPORTED_VERSION,
        });
    }

    // Unique resource IDs.
    let mut seen = std::collections::HashSet::new();
    for (i, resource) in raw.resources.iter().enumerate() {
        if !seen.insert(resource.id.as_str()) {
            return Err(ConfigError::validation(
                format!("resources[{i}].id"),
                format!("duplicate resource id `{}`", resource.id),
            ));
        }
        if resource.id.is_empty() {
            return Err(ConfigError::validation(
                format!("resources[{i}].id"),
                "resource id must not be empty",
            ));
        }
    }
    let known: std::collections::HashSet<&str> =
        raw.resources.iter().map(|r| r.id.as_str()).collect();

    // Alternative groups.
    let mut alternative_groups = Vec::new();
    let mut group_ids = std::collections::HashSet::new();
    for (k, group) in raw.alternative_groups.iter().enumerate() {
        let path = format!("alternative_groups[{k}]");
        if !group_ids.insert(group.id.as_str()) {
            return Err(ConfigError::validation(
                format!("{path}.id"),
                format!("duplicate alternative group id `{}`", group.id),
            ));
        }
        for (m, member) in group.members.iter().enumerate() {
            if !known.contains(member.as_str()) {
                return Err(ConfigError::validation(
                    format!("{path}.members[{m}]"),
                    format!("unknown resource `{member}`"),
                ));
            }
        }
        if group.required == 0 {
            return Err(ConfigError::validation(
                format!("{path}.required"),
                "required must be at least 1",
            ));
        }
        let group = AlternativeGroup {
            group_id: group.id.clone(),
            members: group.members.iter().map(ResourceId::new).collect(),
            required_count: group.required,
        };
        // §3.5 item 5: a group requiring more members than it has can never
        // be satisfied.
        if group.is_impossible() {
            return Err(ConfigError::validation(
                format!("{path}.required"),
                format!(
                    "`required` ({}) exceeds member count ({}) — condition can never be satisfied",
                    group.required_count,
                    group.members.len()
                ),
            ));
        }
        alternative_groups.push(group);
    }

    // Resources: dependencies, plans, then graph.
    let mut resources = Vec::new();
    let mut edges = Vec::new();
    for (i, raw_resource) in raw.resources.iter().enumerate() {
        let base = format!("resources[{i}]");
        let id = ResourceId::new(raw_resource.id.clone());

        // Dependencies (shorthand + explicit).
        let mut dependencies: Vec<Dependency> = Vec::new();
        for (j, target) in raw_resource.depends_on.iter().enumerate() {
            if !known.contains(target.as_str()) {
                return Err(ConfigError::validation(
                    format!("{base}.depends_on[{j}]"),
                    format!("unknown resource `{target}`"),
                ));
            }
            dependencies.push(Dependency::required(ResourceId::new(target.clone())));
        }
        for (j, dep) in raw_resource.dependencies.iter().enumerate() {
            let path = format!("{base}.dependencies[{j}]");
            if !known.contains(dep.resource.as_str()) {
                return Err(ConfigError::validation(
                    format!("{path}.resource"),
                    format!("unknown resource `{}`", dep.resource),
                ));
            }
            let requirement = match dep.requirement.as_deref() {
                None | Some("required") => custos_model::DependencyRequirement::Required,
                Some("optional") => custos_model::DependencyRequirement::Optional,
                Some("conditional") => custos_model::DependencyRequirement::Conditional,
                Some(other) => {
                    return Err(ConfigError::validation(
                        format!("{path}.requirement"),
                        format!(
                            "unknown requirement `{other}` (expected required|optional|conditional)"
                        ),
                    ));
                }
            };
            if requirement == custos_model::DependencyRequirement::Conditional
                && dep.conditional_on.is_none()
            {
                return Err(ConfigError::validation(
                    format!("{path}.conditional_on"),
                    "conditional dependencies must declare `conditional_on`",
                ));
            }
            let condition = dep.condition.clone().unwrap_or(HealthCondition::StatusIs(
                custos_model::HealthStatus::Healthy,
            ));
            if condition.is_impossible() {
                return Err(ConfigError::validation(
                    format!("{path}.condition"),
                    "condition can never be satisfied",
                ));
            }
            dependencies.push(Dependency {
                target_resource_id: ResourceId::new(dep.resource.clone()),
                requirement,
                condition,
                conditional_on: dep.conditional_on.clone(),
                timeout: None,
            });
        }
        edges.push((
            id.clone(),
            dependencies
                .iter()
                .map(|d| d.target_resource_id.clone())
                .collect(),
        ));

        // Recovery plans.
        let mut plans = Vec::new();
        for (p, raw_plan) in raw_resource
            .recovery_policy
            .as_ref()
            .map(|r| r.plans.as_slice())
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let path = format!("{base}.recovery_policy.plans[{p}]");
            if raw_plan.plan_id.is_empty() {
                return Err(ConfigError::validation(
                    format!("{path}.plan_id"),
                    "plan id must not be empty",
                ));
            }
            if raw_plan.max_attempts == 0 {
                return Err(ConfigError::validation(
                    format!("{path}.max_attempts"),
                    "must be at least 1",
                ));
            }
            let risk = raw_plan.risk.unwrap_or(custos_model::RiskLevel::Low);
            if risk == custos_model::RiskLevel::High {
                let allow = raw_resource
                    .recovery_policy
                    .as_ref()
                    .is_some_and(|r| r.allow_high_risk);
                if !allow {
                    return Err(ConfigError::validation(
                        format!("{path}.risk"),
                        "high-risk plans require `allow_high_risk = true` on the resource \
                         (docs/guide/security.md §12.2)",
                    ));
                }
            }
            let verification = raw_plan
                .verification
                .clone()
                .unwrap_or(HealthCondition::StatusIs(
                    custos_model::HealthStatus::Healthy,
                ));
            if verification.is_impossible() {
                return Err(ConfigError::validation(
                    format!("{path}.verification"),
                    "verification condition can never be satisfied",
                ));
            }
            plans.push(RecoveryPlan {
                plan_id: raw_plan.plan_id.clone(),
                applicable_faults: raw_plan.applicable_faults.clone(),
                preconditions: Vec::new(),
                risk_level: risk,
                action: raw_plan.action.clone(),
                timeout_ms: raw_plan.timeout_ms,
                max_attempts: raw_plan.max_attempts,
                cooldown_ms: raw_plan.cooldown_ms,
                idempotent: raw_plan.idempotent,
                verification,
                failure_handling: raw_plan
                    .failure_handling
                    .unwrap_or(custos_model::FailureHandling::TryNextPlan),
                compensation: raw_plan
                    .compensation
                    .as_ref()
                    .map(custos_model::ActionId::new),
            });
        }
        let recovery_policy = RecoveryPolicy {
            plans,
            max_attempts: raw_resource
                .recovery_policy
                .as_ref()
                .and_then(|r| r.max_attempts),
            allow_high_risk: raw_resource
                .recovery_policy
                .as_ref()
                .is_some_and(|r| r.allow_high_risk),
        };

        let health_policy =
            raw_resource
                .health_policy
                .clone()
                .map_or_else(HealthPolicy::default, |h| HealthPolicy {
                    failure_threshold: h.failure_threshold,
                    success_threshold: h.success_threshold,
                });

        let probe_config = raw_resource.probe.clone().map_or_else(
            || serde_json::Value::Null,
            |p| {
                serde_json::json!({
                    "command": p.command,
                    "pid_file": p.pid_file,
                    "interval_ms": p.interval_ms,
                    "timeout_ms": p.timeout_ms,
                    "max_output_bytes": p.max_output_bytes,
                })
            },
        );

        resources.push(Resource {
            id,
            resource_type: raw_resource.resource_type.clone(),
            display_name: raw_resource
                .display_name
                .clone()
                .unwrap_or_else(|| raw_resource.id.clone()),
            plugin_ref: None,
            config: probe_config,
            health_policy,
            recovery_policy,
            dependencies,
            enabled: raw_resource.enabled.unwrap_or(true),
        });
    }

    // Dependency graph: cycle detection with the concrete cycle path.
    let graph = DependencyGraph::build(edges).map_err(|err| match err {
        GraphError::Cycle(path) => ConfigError::validation(
            "dependency graph",
            format!(
                "dependency cycle detected: {}",
                path.iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
        ),
        other => ConfigError::validation("dependency graph", other.to_string()),
    })?;

    Ok(ValidatedConfig {
        resources,
        alternative_groups,
        graph,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
version = 1

[[resources]]
id = "network"
type = "network-connectivity"

[[resources]]
id = "proxy.egress"
type = "proxy"
depends_on = ["network"]

[[resources]]
id = "tunnel"
type = "tunnel"
dependencies = [
  { resource = "proxy.egress", requirement = "required", condition = { status_is = "healthy" } },
]
"#;

    #[test]
    fn accepts_valid_configuration() {
        let config = load(VALID).unwrap();
        assert_eq!(config.resources.len(), 3);
        assert_eq!(config.graph.nodes().count(), 3);
        assert_eq!(config.resources[2].dependencies.len(), 1);
    }

    #[test]
    fn parses_plans_into_recovery_policy() {
        let config = r#"
version = 1

[[resources]]
id = "svc"
type = "custom-command"

[resources.probe]
command = ["true"]

[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["probe_timeout", "process_exited"]
risk = "low"
idempotent = true

[resources.recovery_policy.plans.action]
program = "sh"
args = ["-c", "echo restart"]

[resources.recovery_policy.plans.verification]
probed_within = { seconds = 30 }
"#;
        let config = load(config).unwrap();
        let resource = &config.resources[0];
        assert_eq!(resource.recovery_policy.plans.len(), 1);
        let plan = &resource.recovery_policy.plans[0];
        assert_eq!(plan.plan_id, "restart");
        assert_eq!(
            plan.applicable_faults,
            vec![FaultCode::ProbeTimeout, FaultCode::ProcessExited]
        );
        assert!(plan.idempotent);
        assert_eq!(
            plan.verification,
            HealthCondition::ProbedWithin { seconds: 30 }
        );
        assert_eq!(plan.action.args, vec!["-c", "echo restart"]);
    }

    #[test]
    fn rejects_unknown_fields_instead_of_silently_ignoring() {
        let result = load("version = 1\nunknown_key = true");
        assert!(result.is_err(), "unknown fields must be rejected");
    }

    #[test]
    fn rejects_unsupported_version() {
        let err = load("version = 99").unwrap_err();
        assert!(matches!(
            err,
            ConfigError::UnsupportedVersion {
                found: 99,
                supported: 1
            }
        ));
    }

    #[test]
    fn rejects_duplicate_resource_ids_with_field_path() {
        let config = r#"
version = 1

[[resources]]
id = "proxy"
type = "proxy"

[[resources]]
id = "proxy"
type = "proxy"
"#;
        let err = load(config).unwrap_err();
        assert!(err.to_string().contains("resources[1].id"), "got: {err}");
    }

    #[test]
    fn rejects_dangling_dependency_reference_with_field_path() {
        let config = r#"
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
depends_on = ["ghost"]
"#;
        let err = load(config).unwrap_err();
        assert!(
            err.to_string().contains("depends_on[0]"),
            "error must carry the field path, got: {err}"
        );
    }

    #[test]
    fn rejects_dependency_cycle_at_startup() {
        // The user ruling: on startup the graph must be checked for cycles;
        // a cycle is an error, never an infinite recovery loop.
        let config = r#"
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
depends_on = ["proxy"]

[[resources]]
id = "proxy"
type = "proxy"
depends_on = ["network"]

[[resources]]
id = "network"
type = "network-connectivity"
depends_on = ["tunnel"]
"#;
        let err = load(config).unwrap_err();
        assert!(
            err.to_string().contains("dependency cycle detected"),
            "startup must fail with a cycle error, got: {err}"
        );
        assert!(
            err.to_string().contains("->"),
            "cycle error must show the path, got: {err}"
        );
    }

    #[test]
    fn rejects_self_dependency() {
        let config = r#"
version = 1

[[resources]]
id = "proxy"
type = "proxy"
depends_on = ["proxy"]
"#;
        assert!(load(config).is_err());
    }

    #[test]
    fn rejects_impossible_dependency_condition() {
        let config = r#"
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
dependencies = [
  { resource = "network", condition = { probed_within = { seconds = 0 } } },
]

[[resources]]
id = "network"
type = "network-connectivity"
"#;
        let err = load(config).unwrap_err();
        assert!(
            err.to_string().contains("dependencies[0].condition"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_impossible_alternative_group() {
        let config = r#"
version = 1

[[alternative_groups]]
id = "egress"
members = ["proxy-a"]
required = 2

[[resources]]
id = "proxy-a"
type = "proxy"
"#;
        let err = load(config).unwrap_err();
        assert!(
            err.to_string().contains("alternative_groups[0].required"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_high_risk_plan_without_opt_in() {
        let config = r#"
version = 1

[[resources]]
id = "router"
type = "custom-command"

[[resources.recovery_policy.plans]]
plan_id = "reset-nic"
applicable_faults = ["network_disconnected"]
risk = "high"

[resources.recovery_policy.plans.action]
program = "ip"
args = ["link", "set", "eth0", "down"]
"#;
        let err = load(config).unwrap_err();
        assert!(
            err.to_string().contains("plans[0].risk")
                && err.to_string().contains("allow_high_risk"),
            "got: {err}"
        );
    }

    #[test]
    fn accepts_high_risk_plan_with_explicit_opt_in() {
        let config = r#"
version = 1

[[resources]]
id = "router"
type = "custom-command"

[resources.recovery_policy]
allow_high_risk = true

[[resources.recovery_policy.plans]]
plan_id = "reset-nic"
applicable_faults = ["network_disconnected"]
risk = "high"

[resources.recovery_policy.plans.action]
program = "ip"
args = ["link", "set", "eth0", "down"]
"#;
        assert!(load(config).is_ok());
    }

    #[test]
    fn rejects_conditional_dependency_without_mode_key() {
        let config = r#"
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
dependencies = [{ resource = "network", requirement = "conditional" }]

[[resources]]
id = "network"
type = "network-connectivity"
"#;
        let err = load(config).unwrap_err();
        assert!(err.to_string().contains("conditional_on"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_requirement_kind() {
        let config = r#"
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
dependencies = [{ resource = "network", requirement = "sometimes" }]

[[resources]]
id = "network"
type = "network-connectivity"
"#;
        assert!(load(config).is_err());
    }
}

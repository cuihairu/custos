//! Validated configuration and the load/validate/build path: parses the
//! raw TOML schema, validates references and conditions, then constructs
//! the runtime graph. Reject at configuration stage, never recurse
//! during recovery (docs/guide/resources.md §3.5).

use super::*;
use crate::schema::RawConfig;
use custos_model::{
    AlternativeGroup, Dependency, DependencyGraph, GraphError, HealthCondition, HealthPolicy,
    RecoveryPlan, RecoveryPolicy, Resource, ResourceId,
};

/// Validated configuration ready for the runtime registry.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedConfig {
    pub resources: Vec<Resource>,
    pub alternative_groups: Vec<AlternativeGroup>,
    pub graph: DependencyGraph,
    /// Global authorization defaults.
    pub permissions: Permissions,
    /// Independent notification section (docs/guide/notification.md §8.5).
    pub notification: NotificationConfig,
    /// Optional web dashboard (docs/guide/dashboard-cli-config.md §9.1).
    pub dashboard: DashboardConfig,
}

/// Global authorization defaults carried into the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions {
    pub default_allow_high_risk: bool,
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

    // Global authorization defaults, resolved before per-resource checks so
    // a resource may either opt in itself or inherit the global switch.
    let permissions = Permissions {
        default_allow_high_risk: raw
            .permissions
            .as_ref()
            .is_some_and(|p| p.default_allow_high_risk),
    };

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
            let (requirement, alternative_group_id) = if let Some(group) = dep.group.as_deref() {
                // N-of-M backup group: the group must exist, and `resource`
                // must be one of its members (used only as the graph anchor).
                let group_def = alternative_groups
                    .iter()
                    .find(|g| g.group_id == group)
                    .ok_or_else(|| {
                        ConfigError::validation(
                            format!("{path}.group"),
                            format!("unknown alternative group `{group}`"),
                        )
                    })?;
                if !group_def.members.iter().any(|m| m.as_str() == dep.resource) {
                    return Err(ConfigError::validation(
                        format!("{path}.resource"),
                        format!(
                            "`{}` is not a member of alternative group `{group}`",
                            dep.resource
                        ),
                    ));
                }
                (
                    custos_model::DependencyRequirement::AlternativeGroup,
                    Some(group.to_owned()),
                )
            } else {
                let requirement = match dep.requirement.as_deref() {
                    None | Some("required") => custos_model::DependencyRequirement::Required,
                    Some("optional") => custos_model::DependencyRequirement::Optional,
                    Some("conditional") => custos_model::DependencyRequirement::Conditional,
                    Some(other) => {
                        return Err(ConfigError::validation(
                            format!("{path}.requirement"),
                            format!(
                                "unknown requirement `{other}` (expected required|optional|conditional|group)"
                            ),
                        ));
                    }
                };
                (requirement, None)
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
                alternative_group_id,
                conditional_on: dep.conditional_on.clone(),
                timeout: None,
            });
        }
        edges.push((
            id.clone(),
            dependencies
                .iter()
                // Alternative-group edges are N-of-M: they do not make any
                // single member a hard dependency (that would demand all
                // members healthy). Group satisfaction is judged in the
                // engine against `alternative_groups`, not via graph edges.
                .filter(|d| d.requirement != custos_model::DependencyRequirement::AlternativeGroup)
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
                let allow = permissions.default_allow_high_risk
                    || raw_resource
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
        // Compensation must reference a plan that exists in this same
        // resource (a cross-resource rollback is a design error, §6.5).
        for (p, plan) in plans.iter().enumerate() {
            if let Some(comp) = &plan.compensation {
                let target_exists = plans.iter().any(|q| q.plan_id == comp.as_str());
                if !target_exists {
                    return Err(ConfigError::validation(
                        format!("{base}.recovery_policy.plans[{p}].compensation"),
                        format!(
                            "compensation references unknown plan `{comp}` \
                             (must be a plan_id in the same resource)"
                        ),
                    ));
                }
            }
        }
        let recovery_policy = RecoveryPolicy {
            plans,
            max_attempts: raw_resource
                .recovery_policy
                .as_ref()
                .and_then(|r| r.max_attempts),
            allow_high_risk: permissions.default_allow_high_risk
                || raw_resource
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
                // Carry the full probe table verbatim so the native probes
                // (http/tcp/dns/tls-cert/disk/mem/cpu/log/docker/icmp/proxy/
                // database/systemd) see their own fields at runtime.
                serde_json::to_value(p).unwrap_or(serde_json::Value::Null)
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

    // Notification section, validated against the known resource set.
    let notification = notification::build(raw.notification, &known).map_err(|err| match err {
        ConfigError::Validation { .. } => err,
        other => ConfigError::validation("notification", other.to_string()),
    })?;

    // Optional dashboard; a missing section means disabled.
    let dashboard = dashboard::parse(raw.dashboard)?;

    Ok(ValidatedConfig {
        resources,
        alternative_groups,
        graph,
        permissions,
        notification,
        dashboard,
    })
}

//! Raw configuration schema: the TOML surface plus the probe field
//! union, as written per resource.

use super::*;
use custos_model::{ActionSpec, FaultCode, HealthCondition};
use serde::{Deserialize, Serialize};

/// Probe wiring as written per resource; carried into `Resource::config`.
///
/// The native probes (survey §26.11) read resource-type-specific fields from
/// the same `[resources.probe]` table. Every field the builtin probes can
/// consume is declared here so a probe table passes `custos validate`; the
/// fields are carried through verbatim into `Resource::config` (§10.3:
/// unknown fields are still errors).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawProbe {
    /// `custom-command`: check command argv.
    #[serde(default)]
    pub(crate) command: Option<Vec<String>>,
    /// `process`: file containing the PID to watch.
    #[serde(default)]
    pub(crate) pid_file: Option<String>,
    #[serde(default = "default_interval_ms")]
    pub(crate) interval_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub(crate) timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    pub(crate) max_output_bytes: usize,
    // http-endpoint.
    #[serde(default)]
    pub(crate) url: Option<String>,
    #[serde(default)]
    pub(crate) fail_if_matches: Option<String>,
    #[serde(default)]
    pub(crate) contains: Option<String>,
    #[serde(default)]
    pub(crate) allowed_statuses: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) max_redirects: Option<u64>,
    #[serde(default)]
    pub(crate) max_response_bytes: Option<u64>,
    #[serde(default)]
    pub(crate) response_time_threshold_ms: Option<u64>,
    #[serde(default)]
    pub(crate) follow_redirects: Option<bool>,
    #[serde(default)]
    pub(crate) verify_tls: Option<bool>,
    // network-connectivity / tls-certificate / icmp / database.
    #[serde(default)]
    pub(crate) host: Option<String>,
    #[serde(default)]
    pub(crate) port: Option<u64>,
    #[serde(default)]
    pub(crate) tls: Option<bool>,
    #[serde(default)]
    pub(crate) server_name: Option<String>,
    #[serde(default)]
    pub(crate) warn_days: Option<u64>,
    #[serde(default)]
    pub(crate) crit_days: Option<u64>,
    #[serde(default)]
    pub(crate) fallback_port: Option<u64>,
    #[serde(default)]
    pub(crate) tcp_fallback: Option<bool>,
    #[serde(default)]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) query: Option<String>,
    #[serde(default)]
    pub(crate) user: Option<String>,
    #[serde(default)]
    pub(crate) password: Option<String>,
    #[serde(default)]
    pub(crate) database: Option<String>,
    // dns.
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) server: Option<String>,
    #[serde(default)]
    pub(crate) record_type: Option<String>,
    #[serde(default)]
    pub(crate) expect: Option<String>,
    // proxy.
    #[serde(default)]
    pub(crate) proxy: Option<String>,
    #[serde(default)]
    pub(crate) target: Option<String>,
    #[serde(default)]
    pub(crate) username: Option<String>,
    // service / systemd.
    #[serde(default)]
    pub(crate) unit: Option<String>,
    // disk / memory / cpu / log / docker / network-interface.
    #[serde(default)]
    pub(crate) mount_point: Option<String>,
    #[serde(default)]
    pub(crate) warn_pct: Option<u64>,
    #[serde(default)]
    pub(crate) crit_pct: Option<u64>,
    #[serde(default)]
    pub(crate) check_swap: Option<bool>,
    #[serde(default)]
    pub(crate) warn_load: Option<f64>,
    #[serde(default)]
    pub(crate) crit_load: Option<f64>,
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) pattern: Option<String>,
    #[serde(default)]
    pub(crate) max_bytes_per_check: Option<u64>,
    #[serde(default)]
    pub(crate) container: Option<String>,
    #[serde(default)]
    pub(crate) max_restarts: Option<u64>,
    #[serde(default)]
    pub(crate) interface: Option<String>,
    #[serde(default)]
    pub(crate) connect_port: Option<u64>,
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
pub(super) struct RawDependency {
    pub(crate) resource: String,
    #[serde(default)]
    pub(crate) requirement: Option<String>,
    /// Alternative-group reference: when set, the edge is an N-of-M
    /// `AlternativeGroup` dependency and `requirement` is implied.
    #[serde(default)]
    pub(crate) group: Option<String>,
    #[serde(default)]
    pub(crate) condition: Option<HealthCondition>,
    #[serde(default)]
    pub(crate) conditional_on: Option<String>,
}

/// One candidate recovery plan as written in the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPlan {
    pub(crate) plan_id: String,
    pub(crate) applicable_faults: Vec<FaultCode>,
    #[serde(default)]
    pub(crate) risk: Option<custos_model::RiskLevel>,
    pub(crate) action: ActionSpec,
    #[serde(default = "default_plan_timeout_ms")]
    pub(crate) timeout_ms: u64,
    #[serde(default = "default_plan_attempts")]
    pub(crate) max_attempts: u32,
    #[serde(default = "default_plan_cooldown_ms")]
    pub(crate) cooldown_ms: u64,
    #[serde(default)]
    pub(crate) idempotent: bool,
    #[serde(default)]
    pub(crate) verification: Option<HealthCondition>,
    #[serde(default)]
    pub(crate) failure_handling: Option<custos_model::FailureHandling>,
    #[serde(default)]
    pub(crate) compensation: Option<String>,
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
pub(super) struct RawResource {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) resource_type: String,
    #[serde(default)]
    pub(crate) display_name: Option<String>,
    #[serde(default)]
    pub(crate) enabled: Option<bool>,
    #[serde(default)]
    pub(crate) probe: Option<RawProbe>,
    #[serde(default)]
    pub(crate) health_policy: Option<PartialHealthPolicy>,
    #[serde(default)]
    pub(crate) recovery_policy: Option<RawRecoveryPolicy>,
    /// Shorthand for `dependencies` with `required` requirement.
    #[serde(default)]
    pub(crate) depends_on: Vec<String>,
    #[serde(default)]
    pub(crate) dependencies: Vec<RawDependency>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialHealthPolicy {
    #[serde(default = "default_failure_threshold")]
    pub(crate) failure_threshold: u32,
    #[serde(default = "default_success_threshold")]
    pub(crate) success_threshold: u32,
}

fn default_failure_threshold() -> u32 {
    3
}
fn default_success_threshold() -> u32 {
    2
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawRecoveryPolicy {
    #[serde(default)]
    pub(crate) plans: Vec<RawPlan>,
    #[serde(default)]
    pub(crate) max_attempts: Option<u32>,
    #[serde(default)]
    pub(crate) allow_high_risk: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawAlternativeGroup {
    pub(crate) id: String,
    pub(crate) members: Vec<String>,
    #[serde(default = "default_required_count")]
    pub(crate) required: usize,
}

fn default_required_count() -> usize {
    1
}

/// Top-level configuration document. Unknown fields must not be silently
/// interpreted as valid configuration (docs/guide/dashboard-cli-config.md §10.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawConfig {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) permissions: Option<RawPermissions>,
    #[serde(default)]
    pub(crate) resources: Vec<RawResource>,
    #[serde(default)]
    pub(crate) alternative_groups: Vec<RawAlternativeGroup>,
    /// Independent notification section (docs/guide/notification.md §8.5).
    #[serde(default)]
    pub(crate) notification: Option<notification::RawNotification>,
    /// Optional web dashboard, disabled by default (docs/guide/dashboard-cli-config.md §9.1).
    #[serde(default)]
    pub(crate) dashboard: Option<dashboard::RawDashboard>,
}

/// Global authorization defaults (docs/guide/security.md §12.2). Per-resource
/// `recovery_policy.allow_high_risk` overrides the global default.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPermissions {
    #[serde(default)]
    pub(crate) default_allow_high_risk: bool,
}

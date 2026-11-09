use super::*;
use custos_model::{FaultCode, HealthCondition, ResourceId};

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
        err.to_string().contains("plans[0].risk") && err.to_string().contains("allow_high_risk"),
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
fn global_default_allows_high_risk_without_per_resource_opt_in() {
    let config = r#"
version = 1

[permissions]
default_allow_high_risk = true

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
    let validated = load(config).expect("global default permits high risk");
    assert!(validated.permissions.default_allow_high_risk);
    let resource = validated.resource(&ResourceId::new("router")).unwrap();
    assert!(
        resource.recovery_policy.allow_high_risk,
        "resource inherits the global default"
    );
}

#[test]
fn per_resource_opt_in_wins_over_global_false() {
    let config = r#"
version = 1

[permissions]
default_allow_high_risk = false

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
fn rejects_compensation_referencing_unknown_plan() {
    let config = r#"
version = 1

[[resources]]
id = "app"
type = "custom-command"

[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["process_exited"]
compensation = "stop-other-resource"

[resources.recovery_policy.plans.action]
program = "true"

[[resources.recovery_policy.plans]]
plan_id = "stop"
applicable_faults = ["process_exited"]

[resources.recovery_policy.plans.action]
program = "true"
"#;
    let err = load(config).unwrap_err();
    assert!(
        err.to_string().contains("compensation") && err.to_string().contains("stop-other-resource"),
        "got: {err}"
    );
}

#[test]
fn accepts_compensation_referencing_same_resource_plan() {
    let config = r#"
version = 1

[[resources]]
id = "app"
type = "custom-command"

[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["process_exited"]
compensation = "stop"

[resources.recovery_policy.plans.action]
program = "true"

[[resources.recovery_policy.plans]]
plan_id = "stop"
applicable_faults = ["process_exited"]

[resources.recovery_policy.plans.action]
program = "true"
"#;
    let validated = load(config).expect("compensation targets a sibling plan");
    let app = validated.resource(&ResourceId::new("app")).unwrap();
    let restart = app
        .recovery_policy
        .plans
        .iter()
        .find(|p| p.plan_id == "restart")
        .unwrap();
    assert_eq!(
        restart.compensation.as_ref().map(|a| a.as_str()),
        Some("stop")
    );
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

#[test]
fn native_probe_fields_are_accepted_and_carried_through() {
    // §26.11 probes read resource-type-specific fields from the same
    // `[resources.probe]` table; `custos validate` must accept them and
    // the runtime config must carry them verbatim.
    let config = r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[resources.probe]
url = "https://example.com/health"
allowed_statuses = ["2xx", "200-299"]
follow_redirects = true
verify_tls = true
response_time_threshold_ms = 500
max_response_bytes = 65536

[[resources]]
id = "net"
type = "network-connectivity"

[resources.probe]
host = "1.1.1.1"
port = 53
tls = false
server_name = "one.one.one.one"
timeout_ms = 2000

[[resources]]
id = "cert"
type = "tls-certificate"

[resources.probe]
host = "example.com"
port = 443
warn_days = 30
crit_days = 7

[[resources]]
id = "resolve"
type = "dns"

[resources.probe]
name = "api.example.com"
server = "1.1.1.1"
record_type = "AAAA"

[[resources]]
id = "cache"
type = "memory"

[resources.probe]
check_swap = true
warn_pct = 80
crit_pct = 90
"#;
    let cfg = load(config).expect("native probe tables must validate");
    let by_id = |id: &str| {
        cfg.resources
            .iter()
            .find(|r| r.id.as_str() == id)
            .expect("resource exists")
    };
    let web = by_id("web");
    assert_eq!(web.config["url"], "https://example.com/health");
    assert_eq!(web.config["follow_redirects"], true);
    assert_eq!(web.config["response_time_threshold_ms"], 500);
    assert_eq!(web.config["max_response_bytes"], 65536);
    let net = by_id("net");
    assert_eq!(net.config["host"], "1.1.1.1");
    assert_eq!(net.config["port"], 53);
    assert_eq!(net.config["server_name"], "one.one.one.one");
    assert_eq!(net.config["timeout_ms"], 2000);
    let cert = by_id("cert");
    assert_eq!(cert.config["warn_days"], 30);
    assert_eq!(cert.config["crit_days"], 7);
    let resolve = by_id("resolve");
    assert_eq!(resolve.config["record_type"], "AAAA");
    let cache = by_id("cache");
    assert_eq!(cache.config["check_swap"], true);
    assert_eq!(cache.config["warn_pct"], 80);
}

#[test]
fn unknown_probe_field_is_still_rejected() {
    // deny_unknown_fields still applies: a typo in a probe table is a
    // configuration error, not silently dropped.
    let config = r#"
version = 1

[[resources]]
id = "net"
type = "network-connectivity"

[resources.probe]
host = "1.1.1.1"
port = 53
hostt = "typo"
"#;
    let err = load(config).unwrap_err();
    assert!(err.to_string().contains("unknown field"), "got: {err}");
}

#[test]
fn alternative_group_dependency_is_parsed() {
    let config = r#"
version = 1

[[alternative_groups]]
id = "egress"
members = ["proxy-a", "proxy-b"]
required = 1

[[resources]]
id = "proxy-a"
type = "proxy"

[[resources]]
id = "proxy-b"
type = "proxy"

[[resources]]
id = "app"
type = "custom-command"
dependencies = [
  { resource = "proxy-a", group = "egress" },
]
"#;
    let cfg = load(config).expect("group dependency must parse");
    let app = cfg
        .resources
        .iter()
        .find(|r| r.id.as_str() == "app")
        .expect("app resource");
    let dep = &app.dependencies[0];
    assert_eq!(
        dep.requirement,
        custos_model::DependencyRequirement::AlternativeGroup
    );
    assert_eq!(dep.alternative_group_id.as_deref(), Some("egress"));
}

#[test]
fn alternative_group_dependency_rejects_unknown_group() {
    let config = r#"
version = 1

[[resources]]
id = "proxy-a"
type = "proxy"

[[resources]]
id = "app"
type = "custom-command"
dependencies = [
  { resource = "proxy-a", group = "missing" },
]
"#;
    let err = load(config).unwrap_err();
    assert!(
        err.to_string().contains("unknown alternative group"),
        "got: {err}"
    );
}

#[test]
fn alternative_group_dependency_rejects_non_member() {
    let config = r#"
version = 1

[[alternative_groups]]
id = "egress"
members = ["proxy-a"]
required = 1

[[resources]]
id = "proxy-a"
type = "proxy"

[[resources]]
id = "proxy-b"
type = "proxy"

[[resources]]
id = "app"
type = "custom-command"
dependencies = [
  { resource = "proxy-b", group = "egress" },
]
"#;
    let err = load(config).unwrap_err();
    assert!(
        err.to_string()
            .contains("not a member of alternative group"),
        "got: {err}"
    );
}

#[test]
fn dashboard_defaults_to_disabled() {
    let config = load("version = 1\n").unwrap();
    assert!(!config.dashboard.is_enabled());
    assert_eq!(
        config.dashboard.listen_addr(),
        "127.0.0.1:8080".parse().unwrap()
    );
}

#[test]
fn dashboard_parses_enabled_with_listen() {
    let config = load(
        r#"
version = 1

[dashboard]
enabled = true
listen = "127.0.0.1:9090"
"#,
    )
    .unwrap();
    assert!(config.dashboard.is_enabled());
    assert_eq!(
        config.dashboard.listen_addr(),
        "127.0.0.1:9090".parse().unwrap()
    );
}

#[test]
fn dashboard_rejects_invalid_listen() {
    let err = load(
        r#"
version = 1

[dashboard]
enabled = true
listen = "not-an-address"
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("dashboard.listen"), "got: {err}");
}

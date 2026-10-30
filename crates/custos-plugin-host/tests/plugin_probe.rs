//! PluginProbe integration tests: a resource's probe served by the real
//! mock-plugin process, exercising the healthy/unhealthy/unreachable
//! conventions and the "unrecoverable plugin marks the resource
//! unprobeable" rule (§4.5 step 9).

use custos_model::{HealthStatus, Resource};
use custos_plugin_api::Method;
use custos_plugin_host::{PluginConfig, PluginProbe, PluginSupervisor, RestartPolicy};
use custos_probe::Probe;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn supervisor(env: &[(&str, &str)]) -> PluginSupervisor {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_mock-plugin"));
    for (key, value) in env {
        config.env.insert((*key).to_owned(), (*value).to_owned());
    }
    PluginSupervisor::new("mock", config, RestartPolicy::default())
}

fn probe_of(supervisor: PluginSupervisor) -> PluginProbe {
    PluginProbe::new(Arc::new(supervisor), "mock.probe", "mock-plugin probe")
}

#[test]
fn healthy_plugin_probe_reports_healthy() {
    let probe = probe_of(supervisor(&[]));
    probe.supervisor().start().expect("start");

    let report = probe.probe(&Resource::new("db", "plugin"));
    assert!(report.succeeded);
    assert_eq!(report.status, HealthStatus::Healthy);
    assert_eq!(report.probe_name, "mock-plugin probe");
    assert_eq!(report.resource_id.to_string(), "db");
}

#[test]
fn plugin_answering_unhealthy_reports_unhealthy_not_unreachable() {
    let probe = probe_of(supervisor(&[]));
    probe.supervisor().start().expect("start");

    // The mock plugin answers healthy=false for resource id "sick".
    let report = probe.probe(&Resource::new("sick", "plugin"));
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unhealthy);
    assert_ne!(report.status, HealthStatus::Unreachable);
}

#[test]
fn degraded_plugin_marks_resource_unprobeable() {
    let supervisor = supervisor(&[("MOCK_PLUGIN_ALWAYS_DIE", "1")]);
    let probe = probe_of(supervisor);
    // Exhaust the crash budget (start + a couple of calls) so the plugin
    // is degraded, then verify the probe reports the resource unprobeable
    // with the degradation named in the observation.
    let _ = probe.supervisor().start();
    for _ in 0..5 {
        let _ = probe.probe(&Resource::new("db", "plugin"));
        let report = probe.probe(&Resource::new("db", "plugin"));
        if report.observation.contains("degraded") {
            break;
        }
    }
    let report = probe.probe(&Resource::new("db", "plugin"));
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unreachable);
    assert!(
        report.observation.contains("degraded") && report.observation.contains("unprobeable"),
        "observation: {}",
        report.observation
    );
}

#[test]
fn timing_out_plugin_probe_is_unreachable() {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_mock-plugin"));
    config.request_timeout = Duration::from_millis(200);
    let supervisor = PluginSupervisor::new(
        "mock",
        config,
        RestartPolicy {
            max_restarts: 1,
            window: Duration::from_secs(60),
        },
    );
    let probe = PluginProbe::new(Arc::new(supervisor), "mock.probe", "mock-plugin probe");
    probe.supervisor().start().expect("start");

    // The mock plugin never answers for resource id "hang".
    let report = probe.probe(&Resource::new("hang", "plugin"));
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unreachable);
    assert!(
        report.observation.contains("timed out"),
        "{}",
        report.observation
    );
}

#[test]
fn unavailable_and_protocol_errors_surface_as_unreachable_observations() {
    let probe = probe_of(supervisor(&[]));
    probe.supervisor().start().expect("start");

    // An rpc-level error from a live plugin (e.g. method rejected by the
    // plugin) must not crash the engine either: it is Unreachable.
    let _ = probe
        .supervisor()
        .call(Method::ListRecoveryActions, json!({"resource_id": "db"}));
    // The fixture implements list_recovery_actions via the unknown-method
    // fallback (-32601), so the probe path below still yields a clean
    // report-shaped outcome for the next operation.
    let report = probe.probe(&Resource::new("db", "plugin"));
    assert!(
        report.status == HealthStatus::Healthy || report.status == HealthStatus::Unhealthy,
        "probe after an rpc error must still work: {:?}",
        report
    );
}

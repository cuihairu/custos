//! Drives the real openclash plugin binary through the host session.
//! No core runs in CI: probe answers healthy=false (core unreachable);
//! switching without a group refuses with a clear detail.

use custos_plugin_host::{PluginConfig, PluginSession};
use std::time::Duration;

fn config() -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_openclash-plugin"));
    config
        .env
        .insert("OPENCLASH_API".into(), "http://127.0.0.1:1".into());
    config.request_timeout = Duration::from_secs(8);
    config
}

#[test]
fn handshake_probe_and_ordered_switch_refusal() {
    let session = PluginSession::start(&config()).expect("spawn openclash plugin");
    assert_eq!(
        session.initialize().expect("handshake").protocol_version,
        "1"
    );
    let capabilities = session.capabilities().expect("capabilities");
    assert!(capabilities
        .iter()
        .any(|c| c.operation_id == "openclash.switch_best_node"));

    let probe = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "openclash"}),
            Duration::from_secs(8),
        )
        .expect("probe");
    assert_eq!(probe["healthy"], serde_json::json!(false), "{probe}");

    let action = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "openclash.switch_best_node", "resource_id": "openclash"}),
            Duration::from_secs(8),
        )
        .expect("action");
    assert_eq!(action["succeeded"], serde_json::json!(false), "{action}");
    assert!(
        action["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("OPENCLASH_GROUP"),
        "{}",
        action["detail"]
    );
    session.shutdown().ok();
}

#[test]
fn unknown_operation_is_a_protocol_error_not_an_action() {
    let session = PluginSession::start(&config()).expect("spawn openclash plugin");
    session.initialize().expect("handshake");
    let err = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "openclash.nuke", "resource_id": "openclash"}),
            Duration::from_secs(8),
        )
        .expect_err("unknown operation must be an RPC error");
    // -32601 (method not found semantics): the host maps this to
    // NotStarted — the action never legitimately ran (§6.6).
    assert!(err.to_string().contains("-32601"), "got: {err}");
    session.shutdown().ok();
}

#[test]
fn switch_with_a_group_reports_failure_when_core_is_unreachable() {
    let mut config = config();
    config.env.insert("OPENCLASH_GROUP".into(), "Proxy".into());
    let session = PluginSession::start(&config).expect("spawn openclash plugin");
    session.initialize().expect("handshake");
    let action = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "openclash.switch_best_node", "resource_id": "openclash"}),
            Duration::from_secs(8),
        )
        .expect("action");
    assert_eq!(action["succeeded"], serde_json::json!(false), "{action}");
    assert!(
        action["detail"]
            .as_str()
            .unwrap_or_default()
            .starts_with("controller unreachable"),
        "{}",
        action["detail"]
    );
    session.shutdown().ok();
}

/// LIVE check against a real controller (ignored by default):
///   LIVE_OPENCLASH_API=http://192.168.5.11:9090 \
///   LIVE_OPENCLASH_SECRET=... LIVE_OPENCLASH_GROUP=三毛机场 \
///   cargo test -p custos-openclash-plugin --test plugin -- --ignored --nocapture
/// Read-only: probe through the full host session and print the
/// observation — positive evidence the plugin path works on the router.
#[test]
#[ignore]
fn live_probe_reports_core_and_current_node() {
    let base = std::env::var("LIVE_OPENCLASH_API").expect("LIVE_OPENCLASH_API");
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_openclash-plugin"));
    config.env.insert("OPENCLASH_API".into(), base);
    if let Ok(secret) = std::env::var("LIVE_OPENCLASH_SECRET") {
        config.env.insert("OPENCLASH_SECRET".into(), secret);
    }
    if let Ok(group) = std::env::var("LIVE_OPENCLASH_GROUP") {
        config.env.insert("OPENCLASH_GROUP".into(), group);
    }
    config.request_timeout = Duration::from_secs(8);

    let session = PluginSession::start(&config).expect("spawn openclash plugin");
    let probe = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "openclash"}),
            Duration::from_secs(8),
        )
        .expect("live probe");
    assert_eq!(probe["healthy"], serde_json::json!(true), "{probe}");
    println!("live observation: {}", probe["observation"]);
    assert!(
        probe["observation"]
            .as_str()
            .unwrap_or_default()
            .contains("clash core"),
        "{}",
        probe["observation"]
    );
    session.shutdown().ok();
}

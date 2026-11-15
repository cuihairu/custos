//! Drives the real sunlogin plugin binary through the host session.
//! No sunlogin client runs in CI: probe answers healthy=false with the
//! failing stage named; restart refuses with a clear detail; unknown
//! operations are protocol errors, never executed actions.

use custos_plugin_host::{PluginConfig, PluginSession};
use std::time::Duration;

fn config() -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_sunlogin-plugin"));
    // Point discovery at an empty directory so the probe fails fast at the
    // discovery stage instead of scanning real system logs.
    let empty = std::env::temp_dir().join("custos-sunlogin-it-empty");
    std::fs::create_dir_all(&empty).expect("empty log dir");
    config
        .env
        .insert("SUNLOGIN_LOG_DIR".into(), empty.to_string_lossy().into_owned());
    config.env.insert("SUNLOGIN_SERVICE".into(), "custos-missing-unit".into());
    config.request_timeout = Duration::from_secs(8);
    config
}

#[test]
fn probe_restart_and_unknown_operation_contract() {
    let session = PluginSession::start(&config()).expect("spawn sunlogin plugin");
    assert_eq!(
        session.initialize().expect("handshake").protocol_version,
        "1"
    );
    let capabilities = session.capabilities().expect("capabilities");
    assert!(capabilities.iter().any(|c| c.operation_id == "sunlogin.probe"));
    assert!(capabilities
        .iter()
        .any(|c| c.operation_id == "sunlogin.restart"));

    let probe = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "sunlogin"}),
            Duration::from_secs(8),
        )
        .expect("probe");
    assert_eq!(probe["healthy"], serde_json::json!(false), "{probe}");
    assert!(
        probe["observation"]
            .as_str()
            .unwrap_or_default()
            .contains("not discovered"),
        "{}",
        probe["observation"]
    );

    let restart = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "sunlogin.restart", "resource_id": "sunlogin"}),
            Duration::from_secs(8),
        )
        .expect("restart action");
    assert_eq!(restart["succeeded"], serde_json::json!(false), "{restart}");
    assert!(
        restart["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("custos-missing-unit")
            || restart["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("oray-sunlogin"),
        "{}",
        restart["detail"]
    );

    let err = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "sunlogin.nuke", "resource_id": "sunlogin"}),
            Duration::from_secs(8),
        )
        .expect_err("unknown operation must be an RPC error");
    assert!(err.to_string().contains("-32601"), "got: {err}");
    session.shutdown().ok();
}

#[test]
fn relogin_reports_not_implemented_through_the_session() {
    let session = PluginSession::start(&config()).expect("spawn sunlogin plugin");
    session.initialize().expect("handshake");
    let action = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "sunlogin.relogin", "resource_id": "sunlogin"}),
            Duration::from_secs(8),
        )
        .expect("relogin action");
    assert_eq!(action["succeeded"], serde_json::json!(false), "{action}");
    assert!(
        action["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("not implemented"),
        "{}",
        action["detail"]
    );
    session.shutdown().ok();
}

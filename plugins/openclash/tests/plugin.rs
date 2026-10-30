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

//! Drives the real mihomo plugin binary through the host session
//! (CARGO_BIN_EXE_mihomo-plugin). No controller runs in CI: the probe must
//! answer healthy=false with the concrete unreachable reason, and switching
//! without MIHOMO_GROUP must refuse with a clear detail — never crash.

use custos_plugin_host::{PluginConfig, PluginSession};
use std::time::Duration;

fn config() -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_mihomo-plugin"));
    // A controller port that is guaranteed dead in CI.
    config
        .env
        .insert("MIHOMO_API".into(), "http://127.0.0.1:1".into());
    config.request_timeout = Duration::from_secs(8);
    config
}

#[test]
fn handshake_capabilities_and_probe_answer() {
    let session = PluginSession::start(&config()).expect("spawn mihomo plugin");
    let init = session.initialize().expect("handshake");
    assert_eq!(init.protocol_version, "1");
    let capabilities = session.capabilities().expect("capabilities");
    let ops: Vec<&str> = capabilities
        .iter()
        .map(|c| c.operation_id.as_str())
        .collect();
    assert!(ops.contains(&"mihomo.probe"), "{ops:?}");
    assert!(ops.contains(&"mihomo.switch_node"), "{ops:?}");
    assert!(ops.contains(&"mihomo.restart"), "{ops:?}");

    let result = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "proxy.egress"}),
            Duration::from_secs(8),
        )
        .expect("probe call");
    assert_eq!(result["healthy"], serde_json::json!(false), "{result}");
    assert!(
        result["observation"]
            .as_str()
            .unwrap_or_default()
            .contains("unreachable"),
        "{}",
        result["observation"]
    );
    session.shutdown().ok();
}

#[test]
fn switching_without_a_group_refuses_with_detail() {
    let session = PluginSession::start(&config()).expect("spawn mihomo plugin");
    session.initialize().expect("handshake");
    let result = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "mihomo.switch_node", "resource_id": "proxy.egress"}),
            Duration::from_secs(8),
        )
        .expect("action call");
    assert_eq!(result["succeeded"], serde_json::json!(false), "{result}");
    assert!(
        result["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("MIHOMO_GROUP"),
        "{}",
        result["detail"]
    );
    session.shutdown().ok();
}

//! Drives the real cloudflared plugin binary through the host session.
//! No metrics endpoint runs in CI: the probe answers healthy=false with the
//! concrete unreachable reason; the metrics parser is fixture-tested in the
//! plugin's unit tests.

use custos_plugin_host::{PluginConfig, PluginSession};
use std::time::Duration;

fn config() -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_cloudflared-plugin"));
    config
        .env
        .insert("CLOUDFLARED_METRICS".into(), "http://127.0.0.1:1".into());
    config.request_timeout = Duration::from_secs(8);
    config
}

#[test]
fn handshake_and_probe_answer() {
    let session = PluginSession::start(&config()).expect("spawn cloudflared plugin");
    let init = session.initialize().expect("handshake");
    assert_eq!(init.protocol_version, "1");
    let capabilities = session.capabilities().expect("capabilities");
    assert!(capabilities
        .iter()
        .any(|c| c.operation_id == "cloudflared.restart"));

    let result = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "tunnel.egress"}),
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

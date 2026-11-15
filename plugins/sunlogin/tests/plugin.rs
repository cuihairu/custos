//! Drives the real sunlogin plugin binary through the host session.
//! Host-agnostic contract: pinning the service to a missing unit makes the
//! service layer fail on ANY host (clean CI/dev box or a machine with a
//! real install), so probe answers healthy=false, restart refuses with the
//! pinned name in the detail; unknown operations are protocol errors,
//! never executed actions. The `#[ignore]`d live test verifies positive
//! health on a machine that actually runs sunlogin/AweSun.

use custos_plugin_host::{PluginConfig, PluginSession};
use std::time::Duration;

fn config() -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_sunlogin-plugin"));
    // Pin the service candidates so the failing stage is deterministic on
    // any host, and clear discovery overrides.
    config
        .env
        .insert("SUNLOGIN_SERVICE".into(), "custos-missing-unit".into());
    config.env.insert("SUNLOGIN_CONFIG".into(), "".into());
    config.env.insert("SUNLOGIN_API".into(), "".into());
    config.request_timeout = Duration::from_secs(15);
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
            Duration::from_secs(15),
        )
        .expect("probe");
    // Host-agnostic: the pinned missing service fails the service layer on
    // any host — a clean box fails every layer, a real install still fails
    // the pinned service — so healthy=false holds everywhere.
    assert_eq!(probe["healthy"], serde_json::json!(false), "{probe}");
    let observation = probe["observation"].as_str().unwrap_or_default();
    assert!(observation.contains("custos-missing-unit"), "{}", observation);

    let restart = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "sunlogin.restart", "resource_id": "sunlogin"}),
            Duration::from_secs(30),
        )
        .expect("restart action");
    assert_eq!(restart["succeeded"], serde_json::json!(false), "{restart}");
    assert!(
        restart["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("custos-missing-unit"),
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

/// LIVE restart on the real machine (ignored by default): executes the
/// real `sc stop`/`start` (or systemctl restart) through the plugin and
/// polls until the probe is healthy again. NOTE: restarting a remote-access
/// client drops live sunlogin sessions — run only where that is acceptable
/// (SSH access stays unaffected). Run:
///   cargo test -p custos-sunlogin-plugin --test plugin -- --ignored --nocapture
#[test]
#[ignore]
fn live_restart_recovers_the_real_service() {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_sunlogin-plugin"));
    config.request_timeout = Duration::from_secs(30);
    let session = PluginSession::start(&config).expect("spawn sunlogin plugin");
    session.initialize().expect("handshake");

    let probe = |timeout: Duration| {
        session
            .call_with_timeout(
                custos_plugin_api::Method::Probe,
                serde_json::json!({"resource_id": "sunlogin"}),
                timeout,
            )
            .expect("probe")
    };
    let baseline = probe(Duration::from_secs(20));
    assert_eq!(baseline["healthy"], serde_json::json!(true), "{baseline}");

    let restart = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "sunlogin.restart", "resource_id": "sunlogin"}),
            Duration::from_secs(120),
        )
        .expect("restart action");
    println!("restart outcome: {}", restart);
    assert_eq!(restart["succeeded"], serde_json::json!(true), "{restart}");

    // The service manager accepts the start request while the service is
    // still START_PENDING — poll the probe until the ladder is healthy
    // again (processes + cloud session re-established).
    let mut recovered = false;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(3));
        let verify = probe(Duration::from_secs(20));
        println!("verify: {}", verify["observation"]);
        if verify["healthy"] == serde_json::json!(true) {
            recovered = true;
            break;
        }
    }
    assert!(recovered, "service did not return to healthy after restart");
    session.shutdown().ok();
}

/// LIVE read-only probe on a real machine WITH sunlogin/awesun installed
/// (ignored by default) — positive evidence on the actual deployment.
#[test]
#[ignore]
fn live_probe_reports_the_real_client_state() {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_sunlogin-plugin"));
    config.request_timeout = Duration::from_secs(20);
    let session = PluginSession::start(&config).expect("spawn sunlogin plugin");
    session.initialize().expect("handshake");
    let probe = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "sunlogin"}),
            Duration::from_secs(20),
        )
        .expect("live probe");
    println!("live observation: {}", probe["observation"]);
    assert_eq!(probe["healthy"], serde_json::json!(true), "{probe}");
    let observation = probe["observation"].as_str().unwrap_or_default();
    assert!(observation.contains("RUNNING"), "{}", observation);
    assert!(observation.contains("ESTABLISHED"), "{}", observation);
    // The plugin must never count ITSELF as the sunlogin client (its own
    // binary name contains "sunlogin") — self-detection false positive.
    assert!(
        !observation.contains("sunlogin-plugin"),
        "{}",
        observation
    );
    session.shutdown().ok();
}

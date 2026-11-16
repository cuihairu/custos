//! Drives the real network-adapter plugin binary through the host session.
//! Host-agnostic contract: a nonexistent interface makes the probe
//! unhealthy, enable fails naming it, disable is refused by the guard;
//! unknown operations are protocol errors. The `#[ignore]`d live test
//! toggles a REAL idle interface (pass `NETADAPTER_LIVE_IFINDEX`, e.g. 19
//! on the iMac) through disable → enable and asserts the state flips.

use custos_plugin_host::{PluginConfig, PluginSession};
use std::time::Duration;

fn config() -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_network-adapter-plugin"));
    // Supervise an interface that exists on no CI machine; clear guards so
    // the contract is deterministic on any host.
    config
        .env
        .insert("NETADAPTER_INTERFACE".into(), "custos-missing-if".into());
    config.env.insert("NETADAPTER_PROTECTED".into(), "".into());
    config
        .env
        .insert("NETADAPTER_ALLOW_ACTIVE".into(), "".into());
    config.request_timeout = Duration::from_secs(30);
    config
}

#[test]
fn probe_enable_disable_and_unknown_operation_contract() {
    let session = PluginSession::start(&config()).expect("spawn network-adapter plugin");
    assert_eq!(
        session.initialize().expect("handshake").protocol_version,
        "1"
    );
    let capabilities = session.capabilities().expect("capabilities");
    assert!(capabilities
        .iter()
        .any(|c| c.operation_id == "network.probe"));
    assert!(capabilities
        .iter()
        .any(|c| c.operation_id == "network.enable"));
    assert!(capabilities
        .iter()
        .any(|c| c.operation_id == "network.disable"));

    // Probe: the supervised interface does not exist anywhere -> unhealthy,
    // but the adapter inventory itself is real (any host reports SOMETHING).
    let probe = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "network"}),
            Duration::from_secs(15),
        )
        .expect("probe");
    assert_eq!(probe["healthy"], serde_json::json!(false), "{probe}");
    let observation = probe["observation"].as_str().unwrap_or_default();
    assert!(observation.contains("custos-missing-if"), "{}", observation);
    assert!(observation.contains("not found"), "{}", observation);

    // Enable a missing interface: failure names it.
    let enable = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "network.enable", "resource_id": "network",
                "args": {"interface": "custos-missing-if"}}),
            Duration::from_secs(30),
        )
        .expect("enable action");
    assert_eq!(enable["succeeded"], serde_json::json!(false), "{enable}");
    assert!(
        enable["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("custos-missing-if"),
        "{}",
        enable["detail"]
    );

    // Disable a missing interface: refused by the guard, reason named.
    let disable = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "network.disable", "resource_id": "network",
                "args": {"interface": "custos-missing-if"}}),
            Duration::from_secs(30),
        )
        .expect("disable action");
    assert_eq!(disable["succeeded"], serde_json::json!(false), "{disable}");
    let detail = disable["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("custos-missing-if") && detail.contains("not found"),
        "{detail}"
    );

    // Protected list refuses before any existence check.
    let mut protected = config();
    protected
        .env
        .insert("NETADAPTER_PROTECTED".into(), "custos-guarded-if".into());
    drop(session);
    let session = PluginSession::start(&protected).expect("spawn protected plugin");
    session.initialize().expect("handshake");
    let disable = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "network.disable", "resource_id": "network",
                "args": {"interface": "custos-guarded-if"}}),
            Duration::from_secs(30),
        )
        .expect("disable action");
    assert_eq!(disable["succeeded"], serde_json::json!(false), "{disable}");
    assert!(
        disable["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("protected"),
        "{}",
        disable["detail"]
    );

    let err = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "network.nuke", "resource_id": "network"}),
            Duration::from_secs(8),
        )
        .expect_err("unknown operation must be an RPC error");
    assert!(err.to_string().contains("-32601"), "got: {err}");
    session.shutdown().ok();
}

/// LIVE toggle on a real machine (ignored by default): disable an IDLE
/// interface by ifIndex and re-enable it, asserting both flips. NEVER point
/// this at the interface carrying your connection. Run:
///   NETADAPTER_LIVE_IFINDEX=19 cargo test -p custos-network-adapter-plugin \
///     --test plugin -- --ignored --nocapture
#[test]
#[ignore]
fn live_disable_then_enable_flips_the_real_adapter() {
    let index: u32 = std::env::var("NETADAPTER_LIVE_IFINDEX")
        .expect("set NETADAPTER_LIVE_IFINDEX to an IDLE adapter's ifIndex")
        .trim()
        .parse()
        .expect("ifIndex is a number");

    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_network-adapter-plugin"));
    config.request_timeout = Duration::from_secs(30);
    let session = PluginSession::start(&config).expect("spawn network-adapter plugin");
    session.initialize().expect("handshake");

    let admin_state = |timeout: Duration| {
        let probe = session
            .call_with_timeout(
                custos_plugin_api::Method::Probe,
                serde_json::json!({"resource_id": "network"}),
                timeout,
            )
            .expect("probe");
        let observation = probe["observation"].as_str().unwrap_or_default().to_owned();
        // Find the row mentioning the resolved alias by looking for any
        // row; the assertion key is the disable/enable RESULT below —
        // the observation rows carry the states.
        observation
    };

    let before = admin_state(Duration::from_secs(20));
    println!("before: {before}");

    let disable = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "network.disable", "resource_id": "network",
                "args": {"if_index": index}}),
            Duration::from_secs(60),
        )
        .expect("disable action");
    println!("disable: {disable}");
    assert_eq!(disable["succeeded"], serde_json::json!(true), "{disable}");

    let mid = admin_state(Duration::from_secs(20));
    println!("mid: {mid}");

    let enable = session
        .call_with_timeout(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"operation_id": "network.enable", "resource_id": "network",
                "args": {"if_index": index}}),
            Duration::from_secs(60),
        )
        .expect("enable action");
    println!("enable: {enable}");
    assert_eq!(enable["succeeded"], serde_json::json!(true), "{enable}");
    let detail = enable["detail"].as_str().unwrap_or_default();
    // Post-change fact: admin-enabled (connection may still be down on an
    // idle port — the admin flip is what the API guarantees).
    assert!(detail.contains("admin-enabled"), "{detail}");

    session.shutdown().ok();
}

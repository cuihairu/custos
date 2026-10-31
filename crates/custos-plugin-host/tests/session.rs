//! Integration tests: the host session against a real child process (the
//! `mock-plugin` fixture). These cover the P1 acceptance slice — plugin
//! crash never takes the host down, oversized/malformed stdout lines never
//! wedge the transport, requests always return by their deadline, and
//! stderr is drained and capped without ever being parsed.

use custos_plugin_api::{Method, RpcError};
use custos_plugin_host::{validate_capabilities, HostError, PluginConfig, PluginSession};
use serde_json::json;
use std::time::{Duration, Instant};

fn mock_config(env: &[(&str, &str)]) -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_mock-plugin"));
    for (key, value) in env {
        config.env.insert((*key).to_owned(), (*value).to_owned());
    }
    config
}

#[test]
fn full_lifecycle_handshake_capabilities_probe_shutdown() {
    let session = PluginSession::start(&mock_config(&[])).expect("spawn");
    assert!(session.is_running());

    let init = session.initialize().expect("handshake");
    assert_eq!(init.protocol_version, "1");
    assert_eq!(init.plugin.as_deref(), Some("mock-plugin 0.1"));

    let capabilities = session.capabilities().expect("capabilities");
    assert_eq!(capabilities.len(), 2);
    validate_capabilities(&capabilities, &[]).expect("valid capability set");

    let probe = session
        .call(Method::Probe, json!({"resource_id": "mock"}))
        .expect("probe");
    assert_eq!(probe["healthy"], true);
    assert_eq!(probe["resource_id"], "mock");

    session.shutdown().expect("shutdown");
    assert!(!session.is_running());
}

#[test]
fn incompatible_protocol_version_is_rejected_and_process_reaped() {
    let session =
        PluginSession::start(&mock_config(&[("MOCK_PLUGIN_VERSION", "9")])).expect("spawn");
    let err = session.initialize().expect_err("must reject");
    match err {
        HostError::IncompatibleProtocol { plugin, host } => {
            assert_eq!(plugin, "9");
            assert_eq!(host, "1");
        }
        other => panic!("expected IncompatibleProtocol, got {other:?}"),
    }
    assert!(!session.is_running());
}

#[test]
fn request_deadline_expires_and_session_survives() {
    let session = PluginSession::start(&mock_config(&[])).expect("spawn");
    session.initialize().expect("handshake");

    let err = session
        .call_with_timeout(
            Method::Probe,
            json!({"resource_id": "mock", "hang_ms": 600}),
            Duration::from_millis(200),
        )
        .expect_err("must time out");
    match err {
        HostError::Rpc { code, .. } => assert_eq!(code, RpcError::TIMED_OUT),
        other => panic!("expected TIMED_OUT, got {other:?}"),
    }

    // The very next request works while the abandoned one is still hanging.
    let probe = session
        .call(Method::Probe, json!({"resource_id": "mock"}))
        .expect("session still usable");
    assert_eq!(probe["healthy"], true);

    // When the hung request finally answers, its late response is faulted
    // as abandoned, never delivered as a wrong result.
    let deadline = Instant::now() + Duration::from_secs(3);
    while !session.faults().iter().any(|f| f.contains("abandoned")) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        session.faults().iter().any(|f| f.contains("abandoned")),
        "faults: {:?}",
        session.faults()
    );
}

#[test]
fn cancellation_returns_promptly_and_session_survives() {
    let session = PluginSession::start(&mock_config(&[])).expect("spawn");
    session.initialize().expect("handshake");

    let handle = session
        .spawn_call(
            Method::Probe,
            json!({"resource_id": "mock", "hang_ms": 10_000}),
        )
        .expect("send");
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    handle.cancel();
    let err = handle.wait(Duration::from_secs(2)).expect_err("cancelled");
    match err {
        HostError::Rpc { code, .. } => assert_eq!(code, RpcError::CANCELLED),
        other => panic!("expected CANCELLED, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "cancel must not wait out the hang"
    );

    session
        .call(Method::Probe, json!({"resource_id": "mock"}))
        .expect("session still usable");
}

#[test]
fn plugin_crash_is_isolated_and_reported() {
    let session = PluginSession::start(&mock_config(&[])).expect("spawn");
    session.initialize().expect("handshake");

    let err = session
        .call(Method::Probe, json!({"resource_id": "mock", "die": true}))
        .expect_err("plugin died");
    match err {
        HostError::Unavailable => {}
        HostError::Rpc { code, .. } => assert_eq!(code, RpcError::PLUGIN_UNAVAILABLE),
        other => panic!("expected Unavailable/PLUGIN_UNAVAILABLE, got {other:?}"),
    }
    // The in-flight failure is delivered before the reader thread marks the
    // session dead; poll for the death record.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(reason) = session.death_reason() {
            assert!(reason.contains("exited"), "death reason: {reason}");
            assert!(!session.is_running());
            break;
        }
        assert!(Instant::now() < deadline, "session never recorded a death");
        std::thread::sleep(Duration::from_millis(5));
    }

    // Further calls fail fast; the host process itself is unaffected.
    let err = session
        .call(Method::Probe, json!({"resource_id": "mock"}))
        .expect_err("dead session");
    assert!(matches!(err, HostError::Unavailable));
    // The child's final stderr line races its exit; poll for the drain
    // thread to capture it before asserting.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let tail = session.stderr_tail();
        if tail.contains("dying on purpose") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "session stderr never drained; got {tail:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn malformed_and_oversized_stdout_lines_are_skipped_not_fatal() {
    let mut config = mock_config(&[]);
    config.max_message_bytes = 256;
    let session = PluginSession::start(&config).expect("spawn");
    session.initialize().expect("handshake");

    let probe = session
        .call(
            Method::Probe,
            json!({"resource_id": "mock", "garbage_before": true}),
        )
        .expect("garbage line must not wedge the transport");
    assert_eq!(probe["healthy"], true);

    let probe = session
        .call(
            Method::Probe,
            json!({"resource_id": "mock", "big_before": true}),
        )
        .expect("oversized line must be skipped, framing resynced");
    assert_eq!(probe["healthy"], true);

    let faults = session.faults();
    assert!(
        faults.iter().any(|f| f.contains("unparseable")),
        "faults: {faults:?}"
    );
    assert!(
        faults.iter().any(|f| f.contains("above the 256-byte cap")),
        "faults: {faults:?}"
    );
}

#[test]
fn outbound_request_above_the_cap_is_refused_without_writing() {
    let mut config = mock_config(&[]);
    config.max_message_bytes = 128;
    let session = PluginSession::start(&config).expect("spawn");
    session.initialize().expect("handshake");

    let err = session
        .call(
            Method::Probe,
            json!({"resource_id": "mock", "pad": "a".repeat(200)}),
        )
        .expect_err("over cap");
    match err {
        HostError::Rpc { code, .. } => assert_eq!(code, RpcError::MESSAGE_TOO_LARGE),
        other => panic!("expected MESSAGE_TOO_LARGE, got {other:?}"),
    }

    session
        .call(Method::Probe, json!({"resource_id": "mock"}))
        .expect("session still usable");
}

#[test]
fn chatty_stderr_is_drained_and_capped_never_parsed() {
    let session =
        PluginSession::start(&mock_config(&[("MOCK_PLUGIN_STDERR_LINES", "2000")])).expect("spawn");
    session.initialize().expect("handshake");

    // ~290 KiB of stderr; retention must settle at the 64 KiB cap and end
    // with the final marker line.
    let deadline = Instant::now() + Duration::from_secs(5);
    let tail = loop {
        let tail = session.stderr_tail();
        if tail.contains("stderr-flood-final") || Instant::now() > deadline {
            break tail;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(tail.contains("stderr-flood-final"), "tail: {tail:?}");
    assert!(
        tail.len() <= custos_plugin_api::MAX_STDERR_RETAINED_BYTES,
        "retained {} bytes",
        tail.len()
    );
    assert!(
        session.faults().is_empty(),
        "stderr must never surface as protocol faults: {:?}",
        session.faults()
    );
}

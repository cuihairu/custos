//! Supervisor integration tests against the real mock-plugin process:
//! transparent restart after a crash, crash-loop cutoff, and rolling
//! window recovery.

use custos_plugin_api::{Method, RpcError};
use custos_plugin_host::{HostError, PluginConfig, PluginStatus, PluginSupervisor, RestartPolicy};
use serde_json::json;
use std::time::Duration;

fn mock_config(env: &[(&str, &str)]) -> PluginConfig {
    let mut config = PluginConfig::new(env!("CARGO_BIN_EXE_mock-plugin"));
    for (key, value) in env {
        config.env.insert((*key).to_owned(), (*value).to_owned());
    }
    config
}

fn supervisor(env: &[(&str, &str)], policy: RestartPolicy) -> PluginSupervisor {
    PluginSupervisor::new("mock", mock_config(env), policy)
}

fn probe(supervisor: &PluginSupervisor) -> Result<serde_json::Value, HostError> {
    supervisor.call(Method::Probe, json!({"resource_id": "mock"}))
}

#[test]
fn healthy_plugin_serves_and_reports_running() {
    let supervisor = supervisor(&[], RestartPolicy::default());
    assert_eq!(supervisor.status(), PluginStatus::Stopped);
    supervisor.start().expect("start");
    assert_eq!(supervisor.status(), PluginStatus::Running { restarts: 0 });

    let result = probe(&supervisor).expect("probe");
    assert_eq!(result["healthy"], true);
    supervisor.shutdown();
    assert_eq!(supervisor.status(), PluginStatus::Stopped);
}

#[test]
fn crashed_plugin_is_restarted_transparently_on_next_call() {
    // The latch makes the FIRST process exit at startup (consuming the
    // latch); the second spawn is healthy.
    let latch = std::env::temp_dir().join(format!("custos-mock-latch-{}", std::process::id()));
    let _ = std::fs::remove_file(&latch);
    std::fs::write(&latch, b"latch").expect("write latch");

    let supervisor = supervisor(
        &[(
            "MOCK_PLUGIN_DIE_LATCH",
            latch.to_str().expect("utf8 temp path"),
        )],
        RestartPolicy::default(),
    );
    supervisor.start().expect_err("first spawn must die");
    assert_eq!(supervisor.status(), PluginStatus::Stopped);

    // The next call pays for one restart and succeeds transparently.
    let result = probe(&supervisor).expect("probe after restart");
    assert_eq!(result["healthy"], true);
    assert_eq!(supervisor.status(), PluginStatus::Running { restarts: 1 });
    assert!(supervisor.stderr_tail().contains("dying once at startup"));
}

#[test]
fn crash_loop_is_cut_off_and_reported_as_degraded() {
    let supervisor = supervisor(
        &[("MOCK_PLUGIN_ALWAYS_DIE", "1")],
        RestartPolicy {
            max_restarts: 2,
            window: Duration::from_secs(60),
        },
    );

    supervisor.start().expect_err("spawn 0 dies");
    let err = probe(&supervisor).expect_err("spawn 1 dies");
    assert!(matches!(err, HostError::Unavailable));

    let err = probe(&supervisor).expect_err("spawn 2 dies");
    assert!(matches!(err, HostError::Unavailable));

    // Budget exhausted: fail fast, no further spawn attempts.
    let err = probe(&supervisor).expect_err("degraded");
    assert!(matches!(err, HostError::Unavailable));
    match supervisor.status() {
        PluginStatus::Degraded { reason } => {
            assert!(reason.contains("mock"), "reason: {reason}");
            assert!(reason.contains("2 times"), "reason: {reason}");
        }
        other => panic!("expected Degraded, got {other:?}"),
    }
}

#[test]
fn expired_crash_window_resets_the_budget() {
    // max_restarts: 1 with a short window: the first crash degrades the
    // plugin, but once the window expires the supervisor tries again —
    // and the latch ensures the retry is healthy.
    let latch = std::env::temp_dir().join(format!("custos-mock-latch-w-{}", std::process::id()));
    let _ = std::fs::remove_file(&latch);
    std::fs::write(&latch, b"latch").expect("write latch");

    let supervisor = supervisor(
        &[(
            "MOCK_PLUGIN_DIE_LATCH",
            latch.to_str().expect("utf8 temp path"),
        )],
        RestartPolicy {
            max_restarts: 1,
            window: Duration::from_millis(150),
        },
    );

    supervisor.start().expect_err("first spawn dies"); // crash 1
    let err = probe(&supervisor).expect_err("budget of 1 is spent");
    assert!(matches!(err, HostError::Unavailable));
    assert!(matches!(supervisor.status(), PluginStatus::Degraded { .. }));

    std::thread::sleep(Duration::from_millis(200)); // window expires
    let result = probe(&supervisor).expect("budget reset, retry is healthy");
    assert_eq!(result["healthy"], true);
    assert_eq!(supervisor.status(), PluginStatus::Running { restarts: 1 });
}

#[test]
fn rpc_errors_from_a_live_plugin_do_not_count_as_crashes() {
    // A plugin answering a real error is HEALTHY; only process death
    // feeds the crash budget. Version mismatch fails the handshake — the
    // supervisor reports the error without degrading.
    let supervisor = supervisor(&[("MOCK_PLUGIN_VERSION", "9")], RestartPolicy::default());
    let err = supervisor.start().expect_err("incompatible");
    assert!(matches!(err, HostError::IncompatibleProtocol { .. }));
    assert_eq!(supervisor.status(), PluginStatus::Stopped);
}

#[test]
fn timed_out_requests_do_not_count_as_crashes_either() {
    let mut config = mock_config(&[]);
    config.request_timeout = Duration::from_millis(300);
    let supervisor = PluginSupervisor::new(
        "mock",
        config,
        RestartPolicy {
            max_restarts: 1,
            window: Duration::from_secs(60),
        },
    );
    supervisor.start().expect("start");

    match supervisor.call(
        Method::Probe,
        json!({"resource_id": "mock", "hang_ms": 600}),
    ) {
        Err(HostError::Rpc { code, .. }) => assert_eq!(code, RpcError::TIMED_OUT),
        other => panic!("expected TIMED_OUT, got {other:?}"),
    }

    // Still running, no crash recorded: the very next probe works.
    let result = probe(&supervisor).expect("probe");
    assert_eq!(result["healthy"], true);
    assert_eq!(supervisor.status(), PluginStatus::Running { restarts: 0 });
}

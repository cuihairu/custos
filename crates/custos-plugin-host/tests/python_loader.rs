//! Python script loader integration: a real `python3` child speaking the
//! plugin protocol via the shim (todo.md P1: Python 入向, same contract as
//! native plugins). Skips when no interpreter is available.

use custos_plugin_host::{script_plugin_config, PluginSession, PluginSupervisor, RestartPolicy};
use std::time::Duration;

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn write_script(dir: &std::path::Path, body: &str) -> String {
    let path = dir.join("script.py");
    std::fs::write(&path, body).unwrap();
    path.to_str().unwrap().to_owned()
}

const HEALTHY_SCRIPT: &str = r#"
def probe(params):
    return {"healthy": True, "observation": "script probe ok"}
"#;

const UNHEALTHY_SCRIPT: &str = r#"
def probe(params):
    target = params.get("args", {}).get("target", "")
    return {"healthy": False, "observation": f"{target} is stale"}
"#;

const RECOVERY_SCRIPT: &str = r#"
def probe(params):
    return {"healthy": True, "observation": "ok"}
def recover(params):
    return {"succeeded": True, "detail": "script recovery ran"}
"#;

#[test]
fn python_verify_falls_back_when_undefined() {
    if !python3_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(dir.path(), HEALTHY_SCRIPT);
    let config = script_plugin_config(&script).unwrap();
    let session = PluginSession::start(&config).unwrap();
    session.initialize().unwrap();

    let result = session
        .call_with_timeout(
            custos_plugin_api::Method::Verify,
            serde_json::json!({"resource_id": "r", "args": {}}),
            Duration::from_secs(5),
        )
        .expect("verify fallback");
    assert_eq!(result["healthy"], serde_json::json!(null));
    assert_eq!(result["observation"], "script defines no verify()");
    session.shutdown().ok();
}

#[test]
fn python_probe_roundtrip_healthy() {
    if !python3_available() {
        return; // no interpreter on this machine; skip
    }
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(dir.path(), HEALTHY_SCRIPT);
    let config = script_plugin_config(&script).expect("plugin config");
    let session = PluginSession::start(&config).expect("spawn python shim");
    let init = session.initialize().expect("handshake");
    assert_eq!(init.protocol_version, "1");
    let capabilities = session.capabilities().expect("capabilities");
    assert!(capabilities
        .iter()
        .any(|c| c.operation_id == "python.probe" && c.read_only));

    let result = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "r", "args": {}}),
            Duration::from_secs(5),
        )
        .expect("probe call");
    assert_eq!(result["healthy"], serde_json::json!(true));
    assert_eq!(result["observation"], "script probe ok");
    session.shutdown().ok();
}

#[test]
fn python_unhealthy_and_script_errors_surface() {
    if !python3_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(dir.path(), UNHEALTHY_SCRIPT);
    let config = script_plugin_config(&script).unwrap();
    let session = PluginSession::start(&config).unwrap();
    session.initialize().unwrap();

    let result = session
        .call_with_timeout(
            custos_plugin_api::Method::Probe,
            serde_json::json!({"resource_id": "r", "args": {"target": "cache"}}),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(result["healthy"], serde_json::json!(false));
    assert_eq!(result["observation"], "cache is stale");
    session.shutdown().ok();
}

#[test]
fn python_recovery_action_roundtrip() {
    if !python3_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(dir.path(), RECOVERY_SCRIPT);
    let config = script_plugin_config(&script).unwrap();
    let supervisor = PluginSupervisor::new("python-recover", config, RestartPolicy::default());
    supervisor.start().expect("supervised start");

    let result = supervisor
        .call(
            custos_plugin_api::Method::ExecuteRecoveryAction,
            serde_json::json!({"resource_id": "r", "args": {}}),
        )
        .expect("recover call");
    assert_eq!(result["succeeded"], serde_json::json!(true));
    assert_eq!(result["detail"], "script recovery ran");
    supervisor.shutdown();
}

#[test]
fn broken_script_is_a_start_failure_not_a_hang() {
    if !python3_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(dir.path(), "this is not python syntax ((((");
    let config = script_plugin_config(&script).unwrap();
    // The supervisor reports the crash; the host machinery recovers/reports
    // exactly like for a native plugin.
    let supervisor = PluginSupervisor::new("python-broken", config, RestartPolicy::default());
    let err = supervisor.start().expect_err("broken script must fail");
    // Any host error is fine (spawn/protocol/unavailable); the point is it
    // fails fast instead of hanging or crashing the host.
    eprintln!("broken script start error: {err}");
    supervisor.shutdown();
}

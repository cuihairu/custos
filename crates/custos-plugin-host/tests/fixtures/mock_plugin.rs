//! Test-fixture plugin: speaks the custos/v1 stdio protocol so the
//! integration tests can exercise the host against a real child process.
//!
//! Behaviour switches come from the request params and the environment:
//!
//! - env `MOCK_PLUGIN_VERSION` — protocol version reported at initialize
//!   (default "1"; set to something else to test rejection);
//! - env `MOCK_PLUGIN_DIE_LATCH=<path>` — exit(3) at startup while the file
//!   exists (the file is removed first, so the next spawn is healthy);
//! - env `MOCK_PLUGIN_ALWAYS_DIE=1` — exit(3) on every spawn (crash loop);
//! - env `MOCK_PLUGIN_STDERR_LINES=<n>` — flood stderr with n+1 lines at
//!   startup (tests the draining reader and capped retention);
//! - probe params: `die` (exit the process), `hang_ms` (delay the response
//!   in a spawned thread so the read loop stays live), `garbage_before` /
//!   `big_before` (emit a malformed / oversized line before the real
//!   response).
//!
//! Unknown methods are answered with -32601.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

fn reply(out: &Arc<Mutex<std::io::Stdout>>, id: Value, result: Value) {
    let message = json!({"jsonrpc": "2.0", "id": id, "result": result});
    let mut out = out.lock().unwrap();
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}

fn reply_error(out: &Arc<Mutex<std::io::Stdout>>, id: Value, code: i32, message: &str) {
    let body = json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
    let mut out = out.lock().unwrap();
    let _ = writeln!(out, "{body}");
    let _ = out.flush();
}

fn capabilities() -> Value {
    json!([
        {
            "kind": "probe_provider",
            "operation_id": "mock.probe",
            "summary": "probe the mock resource",
            "read_only": true,
            "required_permission": "probe.read",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": []
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "mock.restart",
            "summary": "restart the mock resource",
            "read_only": false,
            "required_permission": "mock.restart",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": true,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["already_stopped"]
        }
    ])
}

fn main() {
    let version = std::env::var("MOCK_PLUGIN_VERSION").unwrap_or_else(|_| "1".into());
    // Crash-injection switches for supervisor tests: die once only while a
    // latch file exists (removed before dying, so the NEXT spawn is
    // healthy), or die on every spawn.
    if let Ok(latch) = std::env::var("MOCK_PLUGIN_DIE_LATCH") {
        let latch = std::path::PathBuf::from(latch);
        if latch.exists() {
            let _ = std::fs::remove_file(&latch);
            eprintln!("mock plugin dying once at startup");
            std::process::exit(3);
        }
    }
    if std::env::var("MOCK_PLUGIN_ALWAYS_DIE").is_ok() {
        std::process::exit(3);
    }
    if let Ok(count) = std::env::var("MOCK_PLUGIN_STDERR_LINES") {
        let count: usize = count.parse().unwrap_or(0);
        let mut err = std::io::stderr();
        for i in 0..count {
            let _ = writeln!(err, "stderr-flood {i:05} 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        }
        let _ = writeln!(err, "stderr-flood-final");
    }

    let out = Arc::new(Mutex::new(std::io::stdout()));
    let mut stdin = BufReader::new(std::io::stdin());
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let request: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let id = request["id"].clone();
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let params = request["params"].clone();
        match method.as_str() {
            "custos/v1/initialize" => {
                reply(
                    &out,
                    id,
                    json!({"protocol_version": version, "plugin": "mock-plugin 0.1"}),
                );
            }
            "custos/v1/describe_capabilities" => {
                reply(&out, id, capabilities());
            }
            "custos/v1/probe" => {
                let resource_id = params["resource_id"].clone();
                if params["die"].as_bool().unwrap_or(false) {
                    eprintln!("mock plugin dying on purpose");
                    std::process::exit(3);
                }
                if let Some(ms) = params["hang_ms"].as_u64() {
                    // Respond from a side thread so the read loop keeps
                    // accepting work while this probe hangs.
                    let out = out.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                        reply(
                            &out,
                            id,
                            json!({"healthy": true, "resource_id": resource_id}),
                        );
                    });
                    continue;
                }
                // Resource-id switches for the PluginProbe integration
                // tests: "hang" never answers, "sick" answers unhealthy.
                if resource_id == "hang" {
                    let out = out.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        reply(
                            &out,
                            id,
                            json!({"healthy": true, "resource_id": resource_id}),
                        );
                    });
                    continue;
                }
                {
                    let mut guard = out.lock().unwrap();
                    if params["garbage_before"].as_bool().unwrap_or(false) {
                        let _ = writeln!(guard, "{{{{not json at all");
                    }
                    if params["big_before"].as_bool().unwrap_or(false) {
                        let _ = writeln!(guard, "{}", "x".repeat(8192));
                    }
                    let _ = guard.flush();
                }
                let healthy = resource_id != "sick";
                reply(
                    &out,
                    id,
                    json!({"healthy": healthy, "resource_id": resource_id}),
                );
            }
            "custos/v1/shutdown" => {
                reply(&out, id, Value::Null);
                break;
            }
            other => {
                reply_error(&out, id, -32601, &format!("unknown method {other}"));
            }
        }
    }
}

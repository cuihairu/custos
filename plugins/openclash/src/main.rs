//! OpenClash plugin (todo.md 221: 节点诊断 + 有序切换) — one plugin process,
//! one resource loop. OpenClash runs the clash core on OpenWrt; the core
//! exposes the same external-controller API as mihomo, so probing and node
//! switching reuse that surface, while restart goes through OpenWrt's init
//! script (`/etc/init.d/openclash restart`), not systemctl.
//!
//! Ordered recovery (有序切换): plan 1 diagnoses and switches to the best
//! available node by measured delay; plan 2 restarts the service.
//!
//! Configuration via environment (§4.4):
//!   OPENCLASH_API     controller base (default http://127.0.0.1:9090)
//!   OPENCLASH_SECRET  controller bearer   (optional)
//!   OPENCLASH_GROUP   proxy group         (required for switching)

use custos_plugin_api::server::{serve, Reply};
use custos_probe::exec::{run_controlled, ExecResult};
use serde_json::{json, Value};
use std::time::Duration;

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build(),
    )
}

fn api_base() -> String {
    std::env::var("OPENCLASH_API").unwrap_or_else(|_| "http://127.0.0.1:9090".into())
}

fn secret() -> Option<String> {
    std::env::var("OPENCLASH_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
}

fn group() -> Option<String> {
    std::env::var("OPENCLASH_GROUP")
        .ok()
        .filter(|s| !s.is_empty())
}

fn get(path: &str) -> Result<Value, String> {
    let mut request = agent().get(&format!("{}{path}", api_base()));
    if let Some(secret) = secret() {
        request = request.header("Authorization", format!("Bearer {secret}").as_str());
    }
    let response = request
        .call()
        .map_err(|e| format!("core unreachable: {e}"))?;
    let mut body = response.into_body();
    let text = body
        .read_to_string()
        .map_err(|e| format!("core answer unreadable: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("core JSON: {e}"))
}

fn put(path: &str, body: Value) -> Result<(), String> {
    let mut request = agent().put(&format!("{}{path}", api_base()));
    if let Some(secret) = secret() {
        request = request.header("Authorization", format!("Bearer {secret}").as_str());
    }
    match request
        .header("Content-Type", "application/json")
        .send(body.to_string())
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("switch failed: {e}")),
    }
}

/// Probe: the controller answering is healthy; the managed group's current
/// node (with its latest measured delay, when available) makes the
/// observation actionable for 节点诊断.
fn probe() -> (bool, String) {
    match get("/version") {
        Ok(version) => {
            let core = version
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let detail = match group().and_then(|g| {
                get(&format!("/proxies/{g}")).ok().and_then(|p| {
                    p.get("now")
                        .and_then(Value::as_str)
                        .map(|node| (g, node.to_owned()))
                })
            }) {
                Some((group, node)) => format!("group {group} via {node}"),
                None => "no managed group configured".into(),
            };
            (true, format!("clash core {core}, {detail}"))
        }
        Err(err) => (false, err),
    }
}

/// Ordered switch (有序切换): measure every member's delay through the
/// group, then pick the fastest available node — a diagnosis, not a blind
/// round-robin. Delays come from `/group/{group}/delay`; nodes that fail
/// the health URL are skipped.
fn switch_best_node() -> Result<String, String> {
    let group = group().ok_or_else(|| "no OPENCLASH_GROUP configured".to_owned())?;
    let info = get(&format!("/proxies/{group}"))?;
    let current = info
        .get("now")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let members: Vec<String> = info
        .get("all")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(Value::as_str)
                .filter(|m| {
                    **m != current && !m.contains("自动") && **m != *"DIRECT" && **m != *"REJECT"
                })
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if members.is_empty() {
        return Err("no alternative node available".into());
    }
    // Measure delays for all candidates in one group query (clash API).
    let delay_url = "https://www.gstatic.com/generate_204";
    let delays = get(&format!(
        "/group/{group}/delay?url={delay_url}&timeout=3000"
    ))
    .ok();
    let best = match &delays {
        Some(map) => members
            .iter()
            .filter_map(|m| {
                map.get(m)
                    .and_then(Value::as_u64)
                    .map(|delay| (delay, m.clone()))
            })
            .min_by_key(|(delay, _)| *delay),
        None => None,
    };
    let chosen = match best {
        Some((delay, node)) => {
            let detail = format!("{node} (delay {delay} ms)");
            put(&format!("/proxies/{group}"), json!({ "name": node }))?;
            detail
        }
        None => {
            // No delay data (offline core or all unhealthy): take the first
            // candidate and say so — never restart in this plan.
            let node = members[0].clone();
            put(
                &format!("/proxies/{group}"),
                json!({ "name": node.clone() }),
            )?;
            format!("{node} (no delay data)")
        }
    };
    Ok(format!("switched {group} from {current} to {chosen}"))
}

/// Restart via OpenWrt's init script (no systemctl on OpenWrt).
fn restart_service() -> Result<String, String> {
    match run_controlled(
        &["/etc/init.d/openclash".to_owned(), "restart".to_owned()],
        Duration::from_secs(60),
        16 * 1024,
    ) {
        ExecResult::Succeeded { .. } => Ok("/etc/init.d/openclash restart done".into()),
        ExecResult::Failed { code, stderr, .. } => Err(format!("exit {code}: {}", stderr.trim())),
        ExecResult::TimedOut => Err("restart timed out".into()),
        ExecResult::StartFailed(err) => {
            Err(format!("init script not runnable (not OpenWrt?): {err}"))
        }
    }
}

fn capabilities() -> Value {
    json!([
        {
            "kind": "probe_provider",
            "operation_id": "openclash.probe",
            "summary": "clash core health and current node of the managed group",
            "read_only": true,
            "required_permission": "probe.read",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["core_unreachable"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "openclash.switch_best_node",
            "summary": "measure member delays and switch to the fastest node",
            "read_only": false,
            "required_permission": "proxy.switch",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": false,
            "verifiable": true,
            "possible_failures": ["no_alternative", "core_unreachable"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "openclash.restart",
            "summary": "restart the openclash service via /etc/init.d",
            "read_only": false,
            "required_permission": "service.manage",
            "affected_scope": "named_resource",
            "interrupts_network": true,
            "interrupts_remote_admin": true,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["init_script_missing", "restart_timeout"]
        }
    ])
}

fn main() {
    serve(|method, params| match method {
        "custos/v1/initialize" => Reply::Result(json!({
            "protocol_version": "1",
            "plugin": "custos-openclash-plugin"
        })),
        "custos/v1/describe_capabilities" => Reply::Result(capabilities()),
        "custos/v1/list_recovery_actions" => {
            Reply::Result(json!(["openclash.switch_best_node", "openclash.restart"]))
        }
        "custos/v1/probe" => {
            let (healthy, observation) = probe();
            Reply::Result(json!({ "healthy": healthy, "observation": observation }))
        }
        "custos/v1/execute_recovery_action" => {
            let operation = params
                .get("operation_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let outcome = match operation {
                "openclash.switch_best_node" => switch_best_node(),
                "openclash.restart" => restart_service(),
                other => return Reply::Error(-32601, format!("unknown operation: {other}")),
            };
            match outcome {
                Ok(detail) => Reply::Result(json!({ "succeeded": true, "detail": detail })),
                Err(err) => Reply::Result(json!({ "succeeded": false, "detail": err })),
            }
        }
        "custos/v1/verify" => {
            let (healthy, observation) = probe();
            Reply::Result(json!({ "healthy": healthy, "observation": observation }))
        }
        "custos/v1/shutdown" => Reply::Exit(json!({ "ok": true })),
        other => Reply::Error(-32601, format!("unknown method: {other}")),
    })
}

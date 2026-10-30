//! mihomo (Clash.Meta) plugin — one plugin process, one resource loop
//! (todo.md 217: 一插件一资源闭环口径).
//!
//! Probe: the external controller's `/version` (the controller answers only
//! when the core is serving); when a proxy group is configured, the group's
//! current selection is reported in the observation.
//! Recovery: switch the configured group to the next available node first
//! (cheap, no restart); restart the service as the second plan (controlled
//! argv through the core's hardened executor — the plugin never shells out
//! itself, ADR-0015: the plugin is the rule layer).
//!
//! Configuration via environment (clean env per §4.4):
//!   MIHOMO_API      controller base URL   (default http://127.0.0.1:9090)
//!   MIHOMO_SECRET   controller bearer     (optional)
//!   MIHOMO_GROUP    proxy group to manage (optional; enables switching)
//!   MIHOMO_SERVICE  systemd unit name     (default mihomo)

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
    std::env::var("MIHOMO_API").unwrap_or_else(|_| "http://127.0.0.1:9090".into())
}

fn auth() -> Option<String> {
    std::env::var("MIHOMO_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
}

fn group() -> Option<String> {
    std::env::var("MIHOMO_GROUP").ok().filter(|s| !s.is_empty())
}

fn service_unit() -> String {
    std::env::var("MIHOMO_SERVICE").unwrap_or_else(|_| "mihomo".into())
}

fn get(path: &str) -> Result<Value, String> {
    let mut request = agent().get(&format!("{}{path}", api_base()));
    if let Some(secret) = auth() {
        request = request.header("Authorization", format!("Bearer {secret}").as_str());
    }
    let response = request
        .call()
        .map_err(|e| format!("controller unreachable: {e}"))?;
    let mut body = response.into_body();
    let text = body
        .read_to_string()
        .map_err(|e| format!("controller answer unreadable: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("controller JSON: {e}"))
}

fn put(path: &str, body: Value) -> Result<(), String> {
    let mut request = agent().put(&format!("{}{path}", api_base()));
    if let Some(secret) = auth() {
        request = request.header("Authorization", format!("Bearer {secret}").as_str());
    }
    let body_text = body.to_string();
    match request
        .header("Content-Type", "application/json")
        .send(body_text)
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("switch failed: {e}")),
    }
}

/// The proxy probe verdict: the controller answering is healthy; a body
/// naming the selected node makes the observation actionable.
fn probe() -> (bool, String) {
    match get("/version") {
        Ok(version) => {
            let detail = version
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            let selected = group().and_then(|g| {
                get(&format!("/proxies/{g}"))
                    .ok()
                    .and_then(|p| p.get("now").and_then(Value::as_str).map(str::to_owned))
            });
            let group_name = group().unwrap_or_default();
            match selected {
                Some(node) => (
                    true,
                    format!("mihomo {detail}, group {group_name} via {node}"),
                ),
                None => (true, format!("mihomo {detail}")),
            }
        }
        Err(err) => (false, err),
    }
}

/// Recovery plan 1: switch the group to its next member (no restart).
/// Requires the controller; refuses when no group is configured.
fn switch_node() -> Result<String, String> {
    let group = group().ok_or_else(|| "no MIHOMO_GROUP configured for switching".to_owned())?;
    let info = get(&format!("/proxies/{group}"))?;
    let members: Vec<String> = info
        .get("all")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let current = info
        .get("now")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // The next node after the current one (wrap); skip groups and DIRECT
    // candidates we cannot distinguish — pick the next plain member.
    let plain: Vec<&String> = members
        .iter()
        .filter(|m| !m.contains("自动") && *m != &current)
        .collect();
    let next = plain
        .first()
        .ok_or_else(|| "no alternative node available".to_owned())?;
    put(&format!("/proxies/{group}"), json!({ "name": next }))?;
    Ok(format!("switched {group} from {current} to {next}"))
}

/// Recovery plan 2: restart the service through the hardened executor.
fn restart_service() -> Result<String, String> {
    let unit = service_unit();
    match run_controlled(
        &["systemctl".to_owned(), "restart".to_owned(), unit.clone()],
        Duration::from_secs(30),
        16 * 1024,
    ) {
        ExecResult::Succeeded { .. } => Ok(format!("systemctl restart {unit} done")),
        ExecResult::Failed { code, stderr, .. } => Err(format!("exit {code}: {}", stderr.trim())),
        ExecResult::TimedOut => Err("restart timed out".into()),
        ExecResult::StartFailed(err) => Err(err),
    }
}

fn capabilities() -> Value {
    json!([
        {
            "kind": "probe_provider",
            "operation_id": "mihomo.probe",
            "summary": "mihomo external controller health and current node",
            "read_only": true,
            "required_permission": "probe.read",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["controller_unreachable"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "mihomo.switch_node",
            "summary": "switch the managed proxy group to the next node",
            "read_only": false,
            "required_permission": "proxy.switch",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": false,
            "verifiable": true,
            "possible_failures": ["no_alternative", "controller_unreachable"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "mihomo.restart",
            "summary": "restart the mihomo systemd service",
            "read_only": false,
            "required_permission": "service.manage",
            "affected_scope": "named_resource",
            "interrupts_network": true,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["unit_not_loaded", "restart_timeout"]
        }
    ])
}

fn main() {
    serve(|method, params| match method {
        "custos/v1/initialize" => Reply::Result(json!({
            "protocol_version": "1",
            "plugin": "custos-mihomo-plugin"
        })),
        "custos/v1/describe_capabilities" => Reply::Result(capabilities()),
        "custos/v1/list_recovery_actions" => {
            Reply::Result(json!(["mihomo.switch_node", "mihomo.restart"]))
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
                "mihomo.switch_node" => switch_node(),
                "mihomo.restart" => restart_service(),
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

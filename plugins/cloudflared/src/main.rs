//! cloudflared plugin — one plugin process, one resource loop (todo.md 218).
//!
//! Probe: the tunnel's metrics endpoint (`/metrics`, default
//! 127.0.0.1:20241 — cloudflared's standard tunnel metrics port). The
//! `ha_connections` / tunnel gauges distinguish "service up but tunnel
//! down" (a real fault) from "controller unreachable" (probe-level
//! Unreachable is reported through healthy=false with a distinct
//! observation).
//! Recovery: restart the service (controlled argv; the plugin is the rule
//! layer, ADR-0015).
//!
//! Configuration via environment (§4.4 clean env):
//!   CLOUDFLARED_METRICS  metrics base URL (default http://127.0.0.1:20241)
//!   CLOUDFLARED_SERVICE  systemd unit         (default cloudflared)

use custos_plugin_api::server::{serve, Reply};
use custos_probe::exec::{run_controlled, ExecResult};
use serde_json::{json, Value};
use std::time::Duration;

fn metrics_base() -> String {
    std::env::var("CLOUDFLARED_METRICS").unwrap_or_else(|_| "http://127.0.0.1:20241".into())
}

fn service_unit() -> String {
    std::env::var("CLOUDFLARED_SERVICE").unwrap_or_else(|_| "cloudflared".into())
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build(),
    )
}

fn fetch(path: &str) -> Result<String, String> {
    let response = agent()
        .get(&format!("{}{path}", metrics_base()))
        .call()
        .map_err(|e| format!("metrics endpoint unreachable: {e}"))?;
    let mut body = response.into_body();
    body.read_to_string()
        .map_err(|e| format!("metrics unreadable: {e}"))
}

/// Tunnel health from the Prometheus text format: `cloudflared_tunnel_ha_
/// connections` counts live edge connections. Zero connections = tunnel
/// down even when the process runs (todo.md P3: 服务在但隧道断 → 真故障).
fn tunnel_health(metrics: &str) -> (bool, String) {
    let mut connections: Option<f64> = None;
    for line in metrics.lines() {
        if let Some(rest) = line.strip_prefix("cloudflared_tunnel_ha_connections") {
            if let Some(value) = rest.rsplit(' ').next().and_then(|v| v.trim().parse().ok()) {
                connections = Some(value);
            }
        }
    }
    match connections {
        Some(count) if count > 0.0 => (true, format!("tunnel up with {count} edge connection(s)")),
        Some(count) => (
            false,
            format!("process serves metrics but the tunnel has {count} edge connections"),
        ),
        None => (
            true,
            "metrics served; no ha_connections gauge exposed".into(),
        ),
    }
}

fn probe() -> (bool, String) {
    match fetch("/metrics") {
        Ok(metrics) => tunnel_health(&metrics),
        Err(err) => (false, err),
    }
}

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
            "operation_id": "cloudflared.probe",
            "summary": "tunnel edge-connection health from the metrics endpoint",
            "read_only": true,
            "required_permission": "probe.read",
            "affected_scope": "named_resource",
            "interrupts_network": false,
            "interrupts_remote_admin": false,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["metrics_unreachable"]
        },
        {
            "kind": "recovery_action_provider",
            "operation_id": "cloudflared.restart",
            "summary": "restart the cloudflared service",
            "read_only": false,
            "required_permission": "service.manage",
            "affected_scope": "named_resource",
            "interrupts_network": true,
            "interrupts_remote_admin": true,
            "cancelable": false,
            "idempotent": true,
            "verifiable": true,
            "possible_failures": ["unit_not_loaded", "restart_timeout"]
        }
    ])
}

fn main() {
    serve(|method, _params| match method {
        "custos/v1/initialize" => Reply::Result(json!({
            "protocol_version": "1",
            "plugin": "custos-cloudflared-plugin"
        })),
        "custos/v1/describe_capabilities" => Reply::Result(capabilities()),
        "custos/v1/list_recovery_actions" => Reply::Result(json!(["cloudflared.restart"])),
        "custos/v1/probe" => {
            let (healthy, observation) = probe();
            Reply::Result(json!({ "healthy": healthy, "observation": observation }))
        }
        "custos/v1/execute_recovery_action" => match restart_service() {
            Ok(detail) => Reply::Result(json!({ "succeeded": true, "detail": detail })),
            Err(err) => Reply::Result(json!({ "succeeded": false, "detail": err })),
        },
        "custos/v1/verify" => {
            let (healthy, observation) = probe();
            Reply::Result(json!({ "healthy": healthy, "observation": observation }))
        }
        "custos/v1/shutdown" => Reply::Exit(json!({ "ok": true })),
        other => Reply::Error(-32601, format!("unknown method: {other}")),
    })
}

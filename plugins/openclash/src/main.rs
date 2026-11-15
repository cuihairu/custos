//! OpenClash plugin (todo.md 221: 节点诊断 + 有序切换) — one plugin process,
//! one resource loop. OpenClash runs the clash core on OpenWrt; the core
//! exposes the same external-controller API as mihomo, so probing and node
//! switching reuse that surface through the shared `clash-api` client,
//! while restart goes through OpenWrt's init script
//! (`/etc/init.d/openclash restart`), not systemctl.
//!
//! Ordered recovery (有序切换): plan 1 diagnoses and switches to the best
//! available node by measured delay; plan 2 restarts the service.
//! Candidate nodes come from `clash_api::real_candidates`, which filters
//! subscription info members (剩余流量/套餐到期/官网…) that carry healthy
//! delays but are not proxies (2026-11-15 fix).
//!
//! Configuration via environment (§4.4):
//!   OPENCLASH_API     controller base (default http://127.0.0.1:9090)
//!   OPENCLASH_SECRET  controller bearer   (optional)
//!   OPENCLASH_GROUP   proxy group         (required for switching)

use clash_api::{best_by_delay, real_candidates, ClashClient};
use custos_plugin_api::server::{serve, Reply};
use custos_probe::exec::{run_controlled, ExecResult};
use serde_json::{json, Value};
use std::time::Duration;

/// The controller client over this plugin's env configuration. Built per
/// call: env is the configuration surface and may change between plugin
/// restarts, never mid-process.
fn client() -> ClashClient {
    ClashClient::from_env("OPENCLASH")
}

fn group() -> Option<String> {
    std::env::var("OPENCLASH_GROUP")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Probe: the controller answering is healthy; the managed group's current
/// node (with its latest measured delay, when available) makes the
/// observation actionable for 节点诊断.
fn probe() -> (bool, String) {
    let client = client();
    match client.version() {
        Ok(core) => {
            let detail = match group().and_then(|g| {
                client
                    .proxy_group(&g)
                    .ok()
                    .and_then(|p| p.now.map(|node| (g, node)))
            }) {
                Some((group, node)) => format!("group {group} via {node}"),
                None => "no managed group configured".into(),
            };
            (true, format!("clash core {core}, {detail}"))
        }
        Err(err) => (false, err.to_string()),
    }
}

/// Ordered switch (有序切换): measure every member's delay through the
/// group, then pick the fastest *real* node — a diagnosis, not a blind
/// round-robin. Delays come from one `/group/{group}/delay` query; nodes
/// that fail the health URL are absent from the map and skipped. With no
/// delay data at all, the first real candidate is taken and said so —
/// never restart in this plan.
fn switch_best_node() -> Result<String, String> {
    let group = group().ok_or_else(|| "no OPENCLASH_GROUP configured".to_owned())?;
    let client = client();
    let info = client.proxy_group(&group).map_err(|e| e.to_string())?;
    let current = info.now.clone().unwrap_or_default();
    let members = real_candidates(&info);
    if members.is_empty() {
        return Err("no alternative node available".into());
    }
    let delays = client
        .group_delays(&group, "https://www.gstatic.com/generate_204", 3000)
        .ok();
    let empty = std::collections::BTreeMap::new();
    let chosen =
        match best_by_delay(&members, delays.as_ref().unwrap_or(&empty)) {
            Some((node, delay)) => {
                client.select(&group, node).map_err(|e| e.to_string())?;
                format!("{node} (delay {delay} ms)")
            }
            None => {
                let node = members[0].clone();
                client.select(&group, &node).map_err(|e| e.to_string())?;
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
            "summary": "measure member delays and switch to the fastest real node",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Env-var accessors are process-global; every test that reads or sets
    /// OPENCLASH_* vars holds this lock so they cannot observe each other.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A loopback address that is listening on nothing: binding port 0,
    /// noting the assigned port and dropping the listener leaves it closed,
    /// so any connection attempt fails at the transport layer immediately.
    fn dead_port_base() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn probe_reports_unhealthy_when_core_is_down() {
        let _guard = ENV.lock().unwrap();
        std::env::set_var("OPENCLASH_API", dead_port_base());
        let (healthy, observation) = probe();
        assert!(!healthy);
        assert!(
            observation.starts_with("controller unreachable: "),
            "got: {observation}"
        );
        std::env::remove_var("OPENCLASH_API");
    }

    #[test]
    fn switch_best_node_requires_a_configured_group() {
        let _guard = ENV.lock().unwrap();
        std::env::remove_var("OPENCLASH_GROUP");
        let err = switch_best_node().expect_err("no group configured");
        assert_eq!(err, "no OPENCLASH_GROUP configured");
    }

    #[test]
    fn switch_best_node_surfaces_core_unreachable() {
        let _guard = ENV.lock().unwrap();
        std::env::set_var("OPENCLASH_API", dead_port_base());
        std::env::set_var("OPENCLASH_GROUP", "Proxy");
        let err = switch_best_node().expect_err("core down");
        assert!(err.starts_with("controller unreachable: "), "got: {err}");
        std::env::remove_var("OPENCLASH_API");
        std::env::remove_var("OPENCLASH_GROUP");
    }

    #[test]
    fn capabilities_declare_the_full_operation_contract() {
        let caps = capabilities();
        let entries = caps.as_array().expect("capabilities are a list");
        assert_eq!(entries.len(), 3, "probe + two recovery actions: {caps}");

        let ids: Vec<&str> = entries
            .iter()
            .filter_map(|e| e.get("operation_id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids,
            [
                "openclash.probe",
                "openclash.switch_best_node",
                "openclash.restart"
            ]
        );
        // The host gates permissions and interruption risk on these fields.
        let probe = &entries[0];
        assert_eq!(probe["kind"], "probe_provider");
        assert_eq!(probe["read_only"], true);
        let restart = &entries[2];
        assert_eq!(restart["kind"], "recovery_action_provider");
        assert_eq!(restart["interrupts_network"], true);
        assert_eq!(restart["interrupts_remote_admin"], true);
    }

    #[test]
    fn restart_service_fails_cleanly_off_openwrt() {
        // On this host /etc/init.d/openclash does not exist, so the
        // controlled spawn must surface StartFailed, not panic or hang.
        let err = restart_service().expect_err("init script missing");
        assert!(err.starts_with("init script not runnable"), "got: {err}");
    }
}

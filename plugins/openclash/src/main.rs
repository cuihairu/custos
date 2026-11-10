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
    fn env_accessors_default_and_override() {
        let _guard = ENV.lock().unwrap();
        std::env::remove_var("OPENCLASH_API");
        std::env::remove_var("OPENCLASH_SECRET");
        std::env::remove_var("OPENCLASH_GROUP");
        assert_eq!(api_base(), "http://127.0.0.1:9090");
        assert_eq!(secret(), None);
        assert_eq!(group(), None);

        std::env::set_var("OPENCLASH_API", "http://192.168.1.20:9090");
        std::env::set_var("OPENCLASH_SECRET", "s3cret");
        std::env::set_var("OPENCLASH_GROUP", "Proxy");
        assert_eq!(api_base(), "http://192.168.1.20:9090");
        assert_eq!(secret().as_deref(), Some("s3cret"));
        assert_eq!(group().as_deref(), Some("Proxy"));

        std::env::remove_var("OPENCLASH_API");
        std::env::remove_var("OPENCLASH_SECRET");
        std::env::remove_var("OPENCLASH_GROUP");
    }

    #[test]
    fn empty_env_values_are_treated_as_unset() {
        let _guard = ENV.lock().unwrap();
        // An empty SECRET/GROUP must behave as "not configured": a Bearer
        // header of "" would be rejected by the core, and "" is never a
        // valid group name.
        std::env::set_var("OPENCLASH_SECRET", "");
        std::env::set_var("OPENCLASH_GROUP", "");
        assert_eq!(secret(), None);
        assert_eq!(group(), None);
        std::env::remove_var("OPENCLASH_SECRET");
        std::env::remove_var("OPENCLASH_GROUP");
    }

    #[test]
    fn get_maps_transport_failure_to_core_unreachable() {
        let _guard = ENV.lock().unwrap();
        std::env::set_var("OPENCLASH_API", dead_port_base());
        let err = get("/version").expect_err("dead port must fail");
        assert!(
            err.starts_with("core unreachable: "),
            "expected a transport mapping, got: {err}"
        );
        std::env::remove_var("OPENCLASH_API");
    }

    #[test]
    fn put_maps_transport_failure_to_switch_failed() {
        let _guard = ENV.lock().unwrap();
        std::env::set_var("OPENCLASH_API", dead_port_base());
        let err =
            put("/proxies/Proxy", json!({ "name": "node" })).expect_err("dead port must fail");
        assert!(
            err.starts_with("switch failed: "),
            "expected a switch mapping, got: {err}"
        );
        std::env::remove_var("OPENCLASH_API");
    }

    #[test]
    fn probe_reports_unhealthy_when_core_is_down() {
        let _guard = ENV.lock().unwrap();
        std::env::set_var("OPENCLASH_API", dead_port_base());
        let (healthy, observation) = probe();
        assert!(!healthy);
        assert!(
            observation.starts_with("core unreachable: "),
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
        assert!(err.starts_with("core unreachable: "), "got: {err}");
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

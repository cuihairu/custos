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
//! HTTP surface and candidate filtering come from the shared `clash-api`
//! client: `next_plain_member` skips pseudo-nodes (DIRECT/REJECT) and
//! subscription info members (剩余流量/套餐到期…) exactly like the
//! openclash best-node picker (2026-11-15 fix).
//!
//! Configuration via environment (clean env per §4.4):
//!   MIHOMO_API      controller base URL   (default http://127.0.0.1:9090)
//!   MIHOMO_SECRET   controller bearer     (optional)
//!   MIHOMO_GROUP    proxy group to manage (optional; enables switching)
//!   MIHOMO_SERVICE  systemd unit name     (default mihomo)

use clash_api::{is_selectable_node, ClashClient};
use custos_plugin_api::server::{serve, Reply};
use custos_probe::exec::{run_controlled, ExecResult};
use serde_json::{json, Value};
use std::time::Duration;

/// The controller client over this plugin's env configuration.
fn client() -> ClashClient {
    ClashClient::from_env("MIHOMO")
}

fn group() -> Option<String> {
    std::env::var("MIHOMO_GROUP").ok().filter(|s| !s.is_empty())
}

fn service_unit() -> String {
    std::env::var("MIHOMO_SERVICE").unwrap_or_else(|_| "mihomo".into())
}

/// The proxy probe verdict: the controller answering is healthy; a body
/// naming the selected node makes the observation actionable.
fn probe() -> (bool, String) {
    let client = client();
    match client.version() {
        Ok(detail) => {
            let selected = group().and_then(|g| {
                client
                    .proxy_group(&g)
                    .ok()
                    .and_then(|p| p.now.map(|node| node.to_owned()))
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
        Err(err) => (false, err.to_string()),
    }
}

/// The next plain member after `current`, wrapping around the list (pure).
/// Routing directives (DIRECT/REJECT), selector members (自动/自動) and
/// subscription info nodes are skipped via the shared [`is_selectable_node`]
/// filter. Taking the *first* non-current member bounced repeated recovery
/// between two nodes (R15): after switching A→B the next recovery picked A
/// again. Walking from the position after `current` advances the rotation
/// A→B→C→A.
fn next_plain_member(members: &[String], current: &str) -> Option<String> {
    let n = members.len();
    if n == 0 {
        return None;
    }
    let start = members.iter().position(|m| m == current).unwrap_or(0);
    for step in 1..=n {
        let candidate = &members[(start + step) % n];
        if candidate != current && is_selectable_node(candidate) {
            return Some(candidate.clone());
        }
    }
    None
}

/// Recovery plan 1: switch the group to its next member (no restart).
/// Requires the controller; refuses when no group is configured.
fn switch_node() -> Result<String, String> {
    let group = group().ok_or_else(|| "no MIHOMO_GROUP configured for switching".to_owned())?;
    let client = client();
    let info = client.proxy_group(&group).map_err(|e| e.to_string())?;
    let current = info.now.clone().unwrap_or_default();
    let next = next_plain_member(&info.all, &current)
        .ok_or_else(|| "no alternative node available".to_owned())?;
    client.select(&group, &next).map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_plain_member_walks_rotation() {
        let members = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        // current=A -> next=B; current=B -> next=C; current=C -> next=A (wrap).
        assert_eq!(next_plain_member(&members, "A").as_deref(), Some("B"));
        assert_eq!(next_plain_member(&members, "B").as_deref(), Some("C"));
        assert_eq!(next_plain_member(&members, "C").as_deref(), Some("A"));
        // Repeated recovery advances the rotation instead of bouncing
        // A<->B on the first non-current member (R15).
        let a = next_plain_member(&members, "A").unwrap();
        let b = next_plain_member(&members, &a).unwrap();
        let c = next_plain_member(&members, &b).unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str()), ("B", "C", "A"));
    }

    #[test]
    fn next_plain_member_skips_selectors_and_self() {
        let members = vec!["自动".to_owned(), "X".to_owned(), "Y".to_owned()];
        assert_eq!(next_plain_member(&members, "X").as_deref(), Some("Y"));
        // Wrapping skips the leading selector.
        assert_eq!(next_plain_member(&members, "Y").as_deref(), Some("X"));
        // Only selectors left -> no alternative.
        let only = vec!["自动".to_owned()];
        assert_eq!(next_plain_member(&only, "自动"), None);
        // The current node is the only plain member -> never switch to self.
        let single = vec!["A".to_owned()];
        assert_eq!(next_plain_member(&single, "A"), None);
        // Empty member list -> no alternative.
        assert_eq!(next_plain_member(&[], "A"), None);
    }

    #[test]
    fn next_plain_member_skips_traditional_selectors_too() {
        // AUD-C: zh-TW selector members use 自動; the doc always promised
        // skipping both spellings — a traditional selector must never be
        // picked as the switch target.
        let members = vec!["X".to_owned(), "自動選擇".to_owned(), "Y".to_owned()];
        assert_eq!(next_plain_member(&members, "X").as_deref(), Some("Y"));
        // Wrapping skips the traditional selector.
        assert_eq!(next_plain_member(&members, "Y").as_deref(), Some("X"));
    }

    #[test]
    fn next_plain_member_skips_pseudo_and_subscription_info_nodes() {
        // 2026-11-15 fix: DIRECT/REJECT and the injected info members
        // (traffic/expiry/homepage) are routing directives or panel data,
        // never switch targets — the old loop could rotate onto them.
        let members = vec![
            "DIRECT".to_owned(),
            "剩余流量：533.08 GB".to_owned(),
            "套餐到期：2027-01-16".to_owned(),
            "官网:example.com".to_owned(),
            "🇸🇬 新加坡 02".to_owned(),
        ];
        assert_eq!(
            next_plain_member(&members, "🇸🇬 新加坡 02").as_deref(),
            None,
            "current node was the only real member"
        );
        let with_alt = vec![
            "DIRECT".to_owned(),
            "剩余流量：533.08 GB".to_owned(),
            "🇸🇬 新加坡 02".to_owned(),
            "🇭🇰 香港 01".to_owned(),
        ];
        assert_eq!(
            next_plain_member(&with_alt, "🇭🇰 香港 01").as_deref(),
            Some("🇸🇬 新加坡 02"),
            "wrap skips DIRECT and info nodes"
        );
    }
}

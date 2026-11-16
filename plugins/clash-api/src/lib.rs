//! Clash / mihomo external-controller client — the one place that knows the
//! REST surface (`/version`, `/proxies/{group}`, `/group/{group}/delay`,
//! `PUT /proxies/{group}`) and the node-selection policy.
//!
//! Both custos proxy plugins (`openclash`, `mihomo`) speak to the same
//! external-controller API — OpenClash runs a clash/mihomo core whose
//! controller is identical — so the client, the percent-encoding of group
//! names (real subscriptions use emoji and spaces: `♻️ 自动选择`) and the
//! subscription info-node filter live here.
//!
//! Real-scenario ruling (2026-11-15): subscription providers inject fake
//! info members into the selector — `剩余流量：533.08 GB`, `套餐到期：
//! 2027-01-16`, `官网 https://…` — which carry *healthy* delay numbers
//! (the controller answers them locally) but are not real proxies. The
//! best-node picker used to select one and break all traffic; everything
//! that picks a node now goes through [`is_subscription_info_node`].

use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// Controller-side failures, mapped so plugins can surface an actionable
/// message without re-stringifying transport details.
#[derive(Debug, thiserror::Error)]
pub enum ClashError {
    /// Transport failure (refused, timeout, DNS): the core is not serving.
    #[error("controller unreachable: {0}")]
    Unreachable(String),
    /// The controller refused the selection (`PUT /proxies/{group}`).
    #[error("switch failed: {0}")]
    Switch(String),
    /// Non-2xx answer from the controller.
    #[error("controller rejected: HTTP {0}")]
    Rejected(u16),
    /// The answer could not be read or parsed.
    #[error("controller answer unreadable: {0}")]
    Unreadable(String),
}

/// `GET /proxies/{group}` payload, restricted to what selection needs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProxyGroup {
    pub name: String,
    /// Selector / URLTest / Fallback — informational, selection is uniform.
    #[serde(rename = "type")]
    pub kind: String,
    /// Current selection (empty for some group types).
    #[serde(default)]
    pub now: Option<String>,
    /// All members, in group order.
    #[serde(default)]
    pub all: Vec<String>,
}

/// Clash / mihomo external-controller client. One client per plugin call
/// cycle is fine: the agent is cheap and config comes from the environment
/// at call time (§4.4: env is the plugin's configuration surface).
pub struct ClashClient {
    agent: ureq::Agent,
    base: String,
    secret: Option<String>,
}

impl ClashClient {
    /// Client with an explicit 5 s global budget: probe and switch must both
    /// stay inside the plugin's per-request watchdog
    /// ([[plugins]].request_timeout_ms, default 30 s).
    pub fn new(base: impl Into<String>, secret: Option<&str>) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(Duration::from_secs(5)))
                // Status codes are data here (401/404 carry different
                // meanings than a refused socket), not errors.
                .http_status_as_error(false)
                .build(),
        );
        Self {
            agent,
            base: base.into().trim_end_matches('/').to_owned(),
            secret: secret.filter(|s| !s.is_empty()).map(str::to_owned),
        }
    }

    /// Reads `{PREFIX}_API` (default `http://127.0.0.1:9090`) and
    /// `{PREFIX}_SECRET` (optional). Empty values count as unset — a Bearer
    /// header of `""` would be rejected by the core.
    pub fn from_env(prefix: &str) -> Self {
        let base = std::env::var(format!("{prefix}_API"))
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:9090".into());
        let secret = std::env::var(format!("{prefix}_SECRET")).ok();
        Self::new(base, secret.as_deref())
    }

    /// Controller base (trimmed), for observations.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// `GET /version` — the controller answering is the minimal health
    /// signal; returns the core version string.
    pub fn version(&self) -> Result<String, ClashError> {
        let value = self.get_json("/version")?;
        Ok(value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned())
    }

    /// `GET /proxies/{group}` — current selection and members.
    pub fn proxy_group(&self, group: &str) -> Result<ProxyGroup, ClashError> {
        let value = self.get_json(&format!("/proxies/{}", encode_path_segment(group)))?;
        serde_json::from_value(value)
            .map_err(|e| ClashError::Unreadable(format!("group payload: {e}")))
    }

    /// `PUT /proxies/{group}` — select `node` in the group.
    pub fn select(&self, group: &str, node: &str) -> Result<(), ClashError> {
        // Only the group name is a path segment; the `/proxies/` prefix must
        // stay literal (encoding it as %2F breaks the URL authority).
        let url = format!("{}/proxies/{}", self.base, encode_path_segment(group));
        let mut request = self.agent.put(&url);
        if let Some(secret) = &self.secret {
            request = request.header("Authorization", &format!("Bearer {secret}"));
        }
        let response = request
            .header("Content-Type", "application/json")
            .send(serde_json::json!({ "name": node }).to_string())
            .map_err(|e| ClashError::Switch(e.to_string()))?;
        self.check_status(response, |status| {
            ClashError::Switch(format!("HTTP {status}"))
        })
        .map(|_| ())
    }

    /// `GET /group/{group}/delay` — measure every member's delay through the
    /// group in one controller round-trip; members that fail `test_url` are
    /// simply absent from the map.
    pub fn group_delays(
        &self,
        group: &str,
        test_url: &str,
        timeout_ms: u64,
    ) -> Result<BTreeMap<String, u64>, ClashError> {
        let path = format!(
            "/group/{}/delay?url={}&timeout={timeout_ms}",
            encode_path_segment(group),
            encode_query_component(test_url),
        );
        let value = self.get_json(&path)?;
        serde_json::from_value(value)
            .map_err(|e| ClashError::Unreadable(format!("delay payload: {e}")))
    }

    fn get_json(&self, path: &str) -> Result<serde_json::Value, ClashError> {
        let url = format!("{}{}", self.base, path);
        let mut request = self.agent.get(&url);
        if let Some(secret) = &self.secret {
            request = request.header("Authorization", &format!("Bearer {secret}"));
        }
        let response = request
            .call()
            .map_err(|e| ClashError::Unreachable(e.to_string()))?;
        let value = self.check_status(response, ClashError::Rejected)?;
        let text = value
            .into_body()
            .read_to_string()
            .map_err(|e| ClashError::Unreadable(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| ClashError::Unreadable(e.to_string()))
    }

    /// Rejects non-2xx with the caller's error mapping before the body is
    /// consumed (401 vs 404 differ: secret vs wrong group).
    fn check_status<T>(
        &self,
        response: ureq::http::Response<T>,
        map: impl Fn(u16) -> ClashError,
    ) -> Result<ureq::http::Response<T>, ClashError> {
        let status = response.status().as_u16();
        if (200..400).contains(&status) {
            Ok(response)
        } else {
            Err(map(status))
        }
    }
}

/// Pseudo-members that are routing directives, never switch targets.
fn is_pseudo_node(name: &str) -> bool {
    matches!(
        name,
        "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "GLOBAL"
    )
}

/// True when a member may serve as a switch target: not a routing
/// directive ([`is_pseudo_node`]) and not subscription info data
/// ([`is_subscription_info_node`]). The one predicate every picker in
/// every plugin goes through.
pub fn is_selectable_node(name: &str) -> bool {
    !is_pseudo_node(name) && !is_subscription_info_node(name)
}

/// Subscription info members (真实订阅注水节点, 2026-11-15): providers render
/// traffic/expiry data as selector members. They pass the delay test but are
/// not proxies — selecting one breaks all traffic. Matches zh-CN and zh-TW
/// spellings plus the common English templates.
pub fn is_subscription_info_node(name: &str) -> bool {
    const KEYWORDS: [&str; 16] = [
        "剩余", "剩餘", // remaining traffic
        "到期", "过期", "過期", // expiry
        "重置", // reset date
        "官网", "官網", // provider homepage
        "流量", "订阅", "訂閱", "套餐", // traffic / subscription / plan
        "自动", "自動", // auto-select group members
        "expire", "traffic",
    ];
    let lower = name.to_lowercase();
    KEYWORDS.iter().any(|k| lower.contains(k))
}

/// Real switch candidates from a group: not the current selection, not
/// pseudo-members, not subscription info nodes. Group order is preserved so
/// fallback selection stays reproducible.
pub fn real_candidates(group: &ProxyGroup) -> Vec<String> {
    group
        .all
        .iter()
        .filter(|m| Some(m.as_str()) != group.now.as_deref())
        .filter(|m| is_selectable_node(m))
        .cloned()
        .collect()
}

/// Fastest member by measured delay; `None` when nobody passed the health
/// URL. First-lowest in group order, so ties are stable and auditable.
pub fn best_by_delay<'a>(
    members: &'a [String],
    delays: &BTreeMap<String, u64>,
) -> Option<(&'a str, u64)> {
    members
        .iter()
        .filter_map(|m| delays.get(m).map(|d| (m.as_str(), *d)))
        .min_by_key(|(_, delay)| *delay)
}

/// Percent-encode a URL path segment (clash group/node names arrive as
/// UTF-8 with emoji, spaces and CJK: `♻️ 自动选择`, `三毛机场`).
pub fn encode_path_segment(segment: &str) -> String {
    encode(segment, false)
}

/// Percent-encode a query value (the health `url` contains `:` and `/`).
pub fn encode_query_component(value: &str) -> String {
    encode(value, true)
}

/// RFC 3986 unreserved bytes pass through; everything else is %-encoded as
/// UTF-8. In query values `+` is encoded too (it means space there).
fn encode(value: &str, query: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        let extra = !query
            && matches!(
                byte,
                b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
            );
        if unreserved || extra {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env-var accessors are process-global; env tests hold this lock.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Fixture in the shape of the real router (2026-11-15): a selector
    /// whose members include pseudo-nodes, the auto group and subscription
    /// info nodes with healthy delays.
    fn selector_fixture() -> serde_json::Value {
        serde_json::json!({
            "name": "三毛机场",
            "type": "Selector",
            "now": "🇭🇰 香港 01",
            "all": [
                "🇭🇰 香港 01",
                "🇸🇬 新加坡 02",
                "DIRECT",
                "REJECT",
                "♻️ 自动选择",
                "剩余流量：533.08 GB",
                "套餐到期：2027-01-16",
                "官网:example.com"
            ]
        })
    }

    #[test]
    fn parses_group_fixture() {
        let group: ProxyGroup = serde_json::from_value(selector_fixture()).expect("fixture parses");
        assert_eq!(group.kind, "Selector");
        assert_eq!(group.now.as_deref(), Some("🇭🇰 香港 01"));
        assert_eq!(group.all.len(), 8);
    }

    #[test]
    fn candidates_exclude_pseudo_auto_and_info_nodes() {
        let group: ProxyGroup = serde_json::from_value(selector_fixture()).expect("fixture parses");
        let candidates = real_candidates(&group);
        // The real bug (2026-11-15): info nodes carry healthy delays and
        // used to win best-node selection. They must never be candidates.
        assert_eq!(candidates, vec!["🇸🇬 新加坡 02".to_owned()]);
    }

    #[test]
    fn info_node_spellings_cover_zh_tw_and_english() {
        for name in [
            "剩余流量：533.08 GB",
            "剩餘流量：533.08 GB",
            "套餐到期：2027-01-16",
            "过期时间：2027-01-16",
            "流量重置日：1",
            "官网 https://example.com",
            "訂閱鏈接",
            "Expire: 2027-01-16",
            "Traffic: 533 GB",
        ] {
            assert!(
                is_subscription_info_node(name),
                "`{name}` must be recognized as an info node"
            );
        }
        // Real node names with benign words stay selectable.
        for name in ["🇭🇰 香港 IEPL 01", "🇯🇵 家宽-东京", "US-Premium"] {
            assert!(
                !is_subscription_info_node(name),
                "`{name}` is a real node, not an info node"
            );
        }
    }

    #[test]
    fn pseudo_nodes_are_routing_directives() {
        assert!(is_pseudo_node("DIRECT"));
        assert!(is_pseudo_node("REJECT"));
        assert!(!is_pseudo_node("🇭🇰 香港 01"));
    }

    #[test]
    fn best_by_delay_prefers_lowest_and_is_stable() {
        let members = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        let delays = BTreeMap::from([("A".to_owned(), 200), ("B".to_owned(), 80)]);
        assert_eq!(best_by_delay(&members, &delays), Some(("B", 80)));
        // Members without a delay measurement (health URL failed) are not
        // candidates even if earlier in group order.
        let none = BTreeMap::from([("A".to_owned(), 200)]);
        assert_eq!(best_by_delay(&members, &none), Some(("A", 200)));
        assert_eq!(best_by_delay(&members, &BTreeMap::new()), None);
    }

    #[test]
    fn path_segments_and_query_values_are_percent_encoded() {
        assert_eq!(
            encode_path_segment("三毛机场"),
            "%E4%B8%89%E6%AF%9B%E6%9C%BA%E5%9C%BA"
        );
        assert_eq!(
            encode_path_segment("♻️ 自动选择"),
            "%E2%99%BB%EF%B8%8F%20%E8%87%AA%E5%8A%A8%E9%80%89%E6%8B%A9"
        );
        assert_eq!(encode_path_segment("US (Premium)"), "US%20(Premium)");
        assert_eq!(
            encode_query_component("https://www.gstatic.com/generate_204"),
            "https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204"
        );
    }

    #[test]
    fn from_env_defaults_and_overrides() {
        let _guard = ENV.lock().unwrap();
        std::env::remove_var("TESTCLASH_API");
        std::env::remove_var("TESTCLASH_SECRET");
        let client = ClashClient::from_env("TESTCLASH");
        assert_eq!(client.base(), "http://127.0.0.1:9090");

        std::env::set_var("TESTCLASH_API", "http://192.168.5.11:9090/");
        std::env::set_var("TESTCLASH_SECRET", "s3cret");
        let client = ClashClient::from_env("TESTCLASH");
        // Trailing slash trimmed so `base + path` never produces `//`.
        assert_eq!(client.base(), "http://192.168.5.11:9090");

        // Empty values count as unset (a `Bearer ` header would be
        // rejected by the core).
        std::env::set_var("TESTCLASH_SECRET", "");
        let client = ClashClient::from_env("TESTCLASH");
        let _ = client;
        std::env::remove_var("TESTCLASH_API");
        std::env::remove_var("TESTCLASH_SECRET");
    }

    #[test]
    fn transport_failure_maps_to_unreachable() {
        let _guard = ENV.lock().unwrap();
        // A loopback address listening on nothing: bind port 0, note the
        // port, drop the listener.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        let client = ClashClient::new(format!("http://127.0.0.1:{port}"), None);
        let err = client.version().expect_err("dead port must fail");
        assert!(
            err.to_string().starts_with("controller unreachable: "),
            "got: {err}"
        );
    }

    #[test]
    fn non_2xx_maps_to_rejected() {
        // A plain TCP "server" that answers 401 for any request.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            drop(stream); // closing without a valid answer → transport error
            let _ = addr;
        });
        let client = ClashClient::new(format!("http://{addr}"), Some("bad"));
        // Either a Rejected(4xx/5xx) or (on abruptly closed connections)
        // Unreachable — both are failures, never a success.
        let result = client.version();
        match result {
            Err(err) => {
                let text = err.to_string();
                assert!(
                    text.starts_with("controller unreachable: ")
                        || text.starts_with("controller rejected: "),
                    "got: {text}"
                );
            }
            Ok(_) => panic!("401 must not pass as success"),
        }
        handle.join().expect("server thread");
    }

    /// LIVE verification against a real controller (2026-11-15 router
    /// scenario). Ignored by default; run with the environment set:
    ///   LIVE_CLASH_API=http://192.168.5.11:9090 \
    ///   LIVE_CLASH_SECRET=... LIVE_CLASH_GROUP=三毛机场 \
    ///   cargo test -p clash-api -- --ignored
    /// Read-only except the final `select` — which puts the CURRENT node
    /// back, so no traffic path changes.
    #[test]
    #[ignore]
    fn live_controller_roundtrip_and_same_node_select() {
        let base = std::env::var("LIVE_CLASH_API").expect("LIVE_CLASH_API");
        let secret = std::env::var("LIVE_CLASH_SECRET").ok();
        let group_name = std::env::var("LIVE_CLASH_GROUP").expect("LIVE_CLASH_GROUP");
        let client = ClashClient::new(base, secret.as_deref());

        let version = client.version().expect("version");
        assert!(!version.is_empty(), "controller version empty");
        println!("controller version: {version}");

        let group = client.proxy_group(&group_name).expect("group");
        let current = group.now.clone().expect("group has a current selection");
        println!(
            "group {} via {current}, {} members",
            group.name,
            group.all.len()
        );

        // Real candidates must exclude the injected info members — assert
        // the policy holds on the LIVE group, not just on fixtures.
        let candidates = real_candidates(&group);
        for name in &candidates {
            assert!(
                is_selectable_node(name),
                "info node `{name}` leaked into candidates: {candidates:?}"
            );
        }
        println!("real candidates ({}): {:?}", candidates.len(), candidates);

        let delays = client
            .group_delays(&group_name, "https://www.gstatic.com/generate_204", 3000)
            .expect("group delay test");
        println!("delay map ({} measured): {:?}", delays.len(), {
            let mut entries: Vec<_> = delays.iter().collect();
            entries.sort_by_key(|(_, d)| **d);
            entries.into_iter().take(5).collect::<Vec<_>>()
        });

        // Write path, non-disruptive: select the current node again.
        client
            .select(&group_name, &current)
            .expect("same-node select");
        let after = client.proxy_group(&group_name).expect("group after select");
        assert_eq!(after.now.as_deref(), Some(current.as_str()));
        println!("select path OK: {current} re-selected (no-op by value)");
    }
}

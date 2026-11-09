use super::net::{chunked_complete, dechunk};
use super::*;
use serde_json::Value;
use std::net::TcpListener;
use std::thread;

fn resource() -> Resource {
    Resource::new("api", "http-endpoint")
}

/// Serves exactly one canned response, then closes the connection.
fn serve_once(response: String) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        // Wait for the request (or the client giving up).
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(response.as_bytes());
    });
    (format!("http://{addr}/"), handle)
}

fn probe_with(url: &str) -> HttpProbe {
    HttpProbe {
        url: url.to_owned(),
        timeout: Duration::from_secs(2),
        allowed_statuses: Vec::new(),
        contains: None,
        fail_if_matches: None,
        response_time_threshold_ms: None,
        follow_redirects: false,
        max_redirects: MAX_REDIRECTS,
        verify_tls: true,
        max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        json_assertions: Vec::new(),
    }
}

// ── JSON assertion tests (survey §26.2 JSON path/value compare) ──────

#[test]
fn json_path_walks_objects_arrays_and_indices() {
    let body = serde_json::json!({
        "status": "ok",
        "count": 42,
        "items": [{ "name": "first" }, { "name": "second" }],
        "nested": { "enabled": true },
        "flag": false
    });
    // Object chain.
    assert_eq!(
        json_path(&body, "$.status").and_then(Value::as_str),
        Some("ok")
    );
    // Without the leading $.
    assert_eq!(json_path(&body, "count").and_then(Value::as_i64), Some(42));
    // Numeric array index.
    assert_eq!(
        json_path(&body, "items.0.name").and_then(Value::as_str),
        Some("first")
    );
    // Nested boolean.
    assert_eq!(
        json_path(&body, "nested.enabled").and_then(Value::as_bool),
        Some(true)
    );
    // Missing path → None.
    assert!(json_path(&body, "missing.key").is_none());
    // Non-numeric index on an array → None.
    assert!(json_path(&body, "items.name").is_none());
    // Object key on a scalar → None.
    assert!(json_path(&body, "status.sub").is_none());
}

#[test]
fn json_assertion_operators_cover_the_matrix() {
    let body = serde_json::json!({
        "status": "ok",
        "count": 42,
        "ratio": 3.5,
        "text": "hello world",
        "enabled": true,
        "nested": { "deep": { "leaf": "found" } },
        "items": [1, 2, 3],
        "empty": null
    });
    let check = |path: &str, op: JsonOp, value: &str| {
        check_json_assertion(
            &body,
            &JsonAssertion {
                path: path.into(),
                op,
                value: value.into(),
            },
        )
    };
    // Equals: string, number, number-as-string (text compare matches),
    // boolean, nested path.
    assert_eq!(check("$.status", JsonOp::Equals, "ok"), None);
    assert_eq!(check("$.count", JsonOp::Equals, "42"), None);
    assert!(check("$.count", JsonOp::Equals, "43").is_some());
    assert_eq!(check("$.nested.deep.leaf", JsonOp::Equals, "found"), None);
    // NotEquals.
    assert_eq!(check("$.status", JsonOp::NotEquals, "error"), None);
    assert!(check("$.status", JsonOp::NotEquals, "ok").is_some());
    // Exists.
    assert_eq!(check("$.items", JsonOp::Exists, ""), None);
    assert!(check("$.missing", JsonOp::Exists, "").is_some());
    // GreaterThan / LessThan: numeric on number and numeric-string.
    assert_eq!(check("$.count", JsonOp::GreaterThan, "10"), None);
    assert!(check("$.count", JsonOp::GreaterThan, "100").is_some());
    assert_eq!(check("$.ratio", JsonOp::LessThan, "4"), None);
    assert!(check("$.ratio", JsonOp::LessThan, "3").is_some());
    // Non-numeric actual must FAIL the numeric op, not pass it
    // (a negated-greater formulation used to let `<` pass silently).
    assert!(check("$.status", JsonOp::LessThan, "5").is_some());
    assert!(check("$.status", JsonOp::GreaterThan, "5").is_some());
    assert!(check("$.enabled", JsonOp::LessThan, "5").is_some());
    // Missing path fails every value operator.
    assert!(check("$.missing", JsonOp::Equals, "x").is_some());
    // null is present: Exists passes; Equals to "null" also matches by
    // JSON text form.
    assert_eq!(check("$.empty", JsonOp::Exists, ""), None);
    // Boolean equality.
    assert_eq!(check("$.enabled", JsonOp::Equals, "true"), None);
    assert!(check("$.enabled", JsonOp::Equals, "false").is_some());
}

#[test]
fn from_config_parses_json_assertions_and_rejects_bad_ops() {
    let config = serde_json::json!({
        "url": "http://x/",
        "json_assertions": [
            { "path": "$.status", "op": "==", "value": "ok" },
            { "path": "$.items", "op": "exists" }
        ]
    });
    let probe = from_config(&config).unwrap();
    assert_eq!(probe.json_assertions.len(), 2);
    assert_eq!(probe.json_assertions[0].path, "$.status");
    assert_eq!(probe.json_assertions[0].op, JsonOp::Equals);
    assert_eq!(probe.json_assertions[1].op, JsonOp::Exists);
    // Bad operator.
    let bad = serde_json::json!({
        "url": "http://x/",
        "json_assertions": [{ "path": "$.a", "op": "~~", "value": "1" }]
    });
    let err = from_config(&bad).unwrap_err();
    assert!(err.contains("invalid"), "{err}");
    // Value required for a value operator.
    let missing = serde_json::json!({
        "url": "http://x/",
        "json_assertions": [{ "path": "$.a", "op": "==" }]
    });
    assert!(from_config(&missing).unwrap_err().contains("required"));
}

#[test]
fn from_config_rejects_redirect_cap_that_truncates() {
    // 2^32 truncates to 0 via `as u32` (redirects silently off); it
    // must be a config error instead.
    let huge = serde_json::json!({ "url": "http://x/", "max_redirects": 4294967296u64 });
    let err = from_config(&huge).unwrap_err();
    assert!(err.contains("max_redirects"), "{err}");
    // A sane explicit cap still parses.
    let sane = serde_json::json!({ "url": "http://x/", "max_redirects": 2 });
    assert_eq!(from_config(&sane).unwrap().max_redirects, 2);
}

#[test]
fn json_assertion_evaluate_fails_on_bad_json_body() {
    let mut probe = probe_with("http://x/");
    probe.json_assertions = vec![JsonAssertion {
        path: "$.status".into(),
        op: JsonOp::Equals,
        value: "ok".into(),
    }];
    let report = probe.evaluate(
        &|succeeded, status, observation: String| ProbeReport {
            resource_id: resource().id,
            probe_name: "http".into(),
            succeeded,
            status,
            observation,
        },
        "http://x/",
        200,
        "<html>not json</html>",
        Duration::from_millis(10),
    );
    assert!(!report.succeeded);
    assert!(
        report.observation.contains("not valid JSON"),
        "{}",
        report.observation
    );
}

#[test]
fn live_json_body_passes_and_fails_assertions() {
    let body = r#"{"status":"ok","version":"1.2","items":[{"name":"a"},{"name":"b"}]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let make_url = || serve_once(response.clone()).0;

    // Passing assertions: string equality + nested path.
    let mut probe = probe_with(&make_url());
    probe.json_assertions = vec![
        JsonAssertion {
            path: "$.status".into(),
            op: JsonOp::Equals,
            value: "ok".into(),
        },
        JsonAssertion {
            path: "items.1.name".into(),
            op: JsonOp::Equals,
            value: "b".into(),
        },
    ];
    let report = probe.probe(&Resource::new("r", "http-endpoint"));
    assert_eq!(
        report.status,
        HealthStatus::Healthy,
        "{}",
        report.observation
    );

    // Failing assertion: version != expected → Unhealthy with the path.
    let mut probe = probe_with(&make_url());
    probe.json_assertions = vec![JsonAssertion {
        path: "$.version".into(),
        op: JsonOp::Equals,
        value: "9.9".into(),
    }];
    let report = probe.probe(&Resource::new("r", "http-endpoint"));
    assert_eq!(report.status, HealthStatus::Unhealthy);
    assert!(
        report.observation.contains("$.version"),
        "{}",
        report.observation
    );
}

#[test]
fn status_and_keyword_pass() {
    let (url, handle) =
        serve_once("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_owned());
    let mut probe = probe_with(&url);
    probe.contains = Some("hello".into());
    let report = probe.probe(&resource());
    assert!(report.succeeded, "{}", report.observation);
    assert_eq!(report.status, HealthStatus::Healthy);
    handle.join().unwrap();
}

#[test]
fn non_2xx_is_unhealthy() {
    let (url, handle) =
        serve_once("HTTP/1.1 503 Unavailable\r\nContent-Length: 5\r\n\r\nlater".to_owned());
    let report = probe_with(&url).probe(&resource());
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unhealthy);
    handle.join().unwrap();
}

#[test]
fn missing_keyword_is_unhealthy() {
    let (url, handle) =
        serve_once("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_owned());
    let mut probe = probe_with(&url);
    probe.contains = Some("absent".into());
    let report = probe.probe(&resource());
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unhealthy);
    handle.join().unwrap();
}

#[test]
fn fail_if_matches_regex_trips() {
    let (url, handle) = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Length: 21\r\n\r\ndegraded: disk almost full".to_owned(),
    );
    let mut probe = probe_with(&url);
    probe.fail_if_matches = Some(Regex::new(r"degraded:\s+disk").unwrap());
    let report = probe.probe(&resource());
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unhealthy);
    handle.join().unwrap();
}

#[test]
fn status_class_range_is_accepted() {
    let (url, handle) = serve_once("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".into());
    let mut probe = probe_with(&url);
    probe.allowed_statuses = vec!["2xx".into()];
    let report = probe.probe(&resource());
    assert!(report.succeeded, "{}", report.observation);
    handle.join().unwrap();
}

#[test]
fn explicit_status_whitelist_rejects_others() {
    let (url, handle) = serve_once("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".into());
    let mut probe = probe_with(&url);
    probe.allowed_statuses = vec!["200".into()];
    let report = probe.probe(&resource());
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unhealthy);
    handle.join().unwrap();
}

#[test]
fn server_that_never_replies_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        // Sleep past the probe's deadline, then drop.
        thread::sleep(Duration::from_millis(500));
    });
    let mut probe = probe_with(&format!("http://{addr}/"));
    probe.timeout = Duration::from_millis(150);
    let report = probe.probe(&resource());
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unreachable);
    assert!(
        report.observation.contains("timeout"),
        "{}",
        report.observation
    );
    handle.join().ok();
}

#[test]
fn slow_response_over_threshold_is_degraded() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        thread::sleep(Duration::from_millis(150));
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    });
    let mut probe = probe_with(&format!("http://{addr}/"));
    probe.response_time_threshold_ms = Some(50);
    let report = probe.probe(&resource());
    assert!(report.succeeded);
    assert_eq!(report.status, HealthStatus::Degraded);
    assert!(
        report.observation.contains("slow"),
        "{}",
        report.observation
    );
    handle.join().unwrap();
}

#[test]
fn redirect_not_followed_by_default() {
    let (url, handle) = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: /other\r\nContent-Length: 0\r\n\r\n".to_owned(),
    );
    let report = probe_with(&url).probe(&resource());
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unhealthy);
    assert!(
        report.observation.contains("not followed"),
        "{}",
        report.observation
    );
    handle.join().unwrap();
}

#[test]
fn redirect_chain_is_followed_when_enabled() {
    // First hop redirects to the second server, which answers 200.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let second = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_owned());
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 301 Moved\r\nLocation: {}\r\nContent-Length: 0\r\n\r\n",
                second.0
            )
            .as_bytes(),
        );
    });
    let mut probe = probe_with(&format!("http://{addr}/"));
    probe.follow_redirects = true;
    let report = probe.probe(&resource());
    assert!(report.succeeded, "{}", report.observation);
    assert_eq!(report.status, HealthStatus::Healthy);
    handle.join().unwrap();
    second.1.join().unwrap();
}

#[test]
fn connection_refused_is_unhealthy() {
    // Bind then drop so the port is closed.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let report = probe_with(&format!("http://127.0.0.1:{port}/")).probe(&resource());
    assert!(!report.succeeded);
    assert_eq!(report.status, HealthStatus::Unreachable);
}

#[test]
fn url_parser_handles_ports_and_ipv6() {
    let parsed = parse_url("https://example.com:8443/health?x=1").unwrap();
    assert_eq!(parsed.scheme, Scheme::Https);
    assert_eq!(parsed.host, "example.com");
    assert_eq!(parsed.port, 8443);
    assert_eq!(parsed.path, "/health?x=1");

    let parsed = parse_url("http://[::1]:8080/x").unwrap();
    assert_eq!(parsed.host, "::1");
    assert_eq!(parsed.port, 8080);

    assert!(parse_url("ftp://x/").is_err());
    assert!(parse_url("no-scheme").is_err());
}

#[test]
fn chunked_body_is_dechunked() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    let response = parse_response(raw, 1024).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "hello world");
}

#[test]
fn huge_chunk_size_is_malformed_not_panic() {
    // A peer-declared chunk size near usize::MAX must overflow-reject
    // (checked math), not panic on the add or slice start > end.
    let body = b"FFFFFFFFFFFFFFFF\r\nabc";
    assert!(!chunked_complete(body));
    assert_eq!(dechunk(body), None);
}

#[test]
fn oversized_but_fitting_chunk_size_is_malformed() {
    // Fits in usize without wrapping yet exceeds the buffered body:
    // rejected by the length guard, not misread as complete.
    let body = b"FFFFFFFF\r\nabc";
    assert!(!chunked_complete(body));
    assert_eq!(dechunk(body), None);
}

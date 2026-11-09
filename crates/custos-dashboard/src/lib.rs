//! Optional read-only web dashboard (docs/guide/dashboard-cli-config.md §9.1).
//!
//! This is an opt-in surface: when `[dashboard].enabled = false` the runtime
//! never binds a listener and no HTTP code path runs. It only reads the
//! engine's SQLite store — it never executes recovery actions, so it cannot
//! bypass Core Engine (ADR-0010, ADR-0012). High-risk actions stay behind the
//! unified permission gate in `custos-toolbox`.
//!
//! The server speaks plain HTTP/1.1 over `std::net::TcpListener` (no external
//! HTTP dependency) and serves a tiny HTML shell plus JSON endpoints:
//! * `/`          — dashboard shell (resources, health, runs, notifications)
//! * `/api/health` — latest probe verdict per resource
//! * `/api/runs`   — recent recovery runs
//! * `/api/notifications` — notification queue counts and entries
//! * `/api/config` — resource configuration check (read-only)

use custos_config::DashboardConfig;
use custos_config::ValidatedConfig;
use custos_store::Store;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

/// One dashboard server bound to the configured listen address. The server is
/// created only when the dashboard is enabled; otherwise no listener exists.
pub struct DashboardServer {
    listener: TcpListener,
    store: Arc<Store>,
    config: ValidatedConfig,
}

impl DashboardServer {
    /// Binds the listener. Call only when `config.is_enabled()`.
    ///
    /// Refuses non-loopback addresses: the endpoints are read-only but
    /// unauthenticated, exposing resource names, recovery plans, risk
    /// levels and notification content. Remote access should go through a
    /// local reverse proxy with its own authentication, not this listener.
    pub fn bind(
        config: &DashboardConfig,
        store: Arc<Store>,
        validated: ValidatedConfig,
    ) -> std::io::Result<Self> {
        let addr = config.listen_addr();
        if !addr.ip().is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "dashboard listen address {addr} is not loopback; refusing to expose \
                     unauthenticated endpoints (use a local reverse proxy for remote access)"
                ),
            ));
        }
        let listener = TcpListener::bind(addr)?;
        Ok(Self {
            listener,
            store,
            config: validated,
        })
    }

    /// The bound local address (useful for logging the actual port).
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Runs the accept loop in the current thread until the listener is
    /// closed. A per-request failure never kills the server: it logs and
    /// continues, keeping the core engine unaffected.
    pub fn serve(self) -> ! {
        let Self {
            listener,
            store,
            config,
        } = self;
        serve_streams(listener.incoming(), store, config)
    }
}

/// The accept loop over any stream source, so tests can drive it with
/// synthetic error streams (C30). Consecutive accept failures back off
/// exponentially instead of busy-spinning (EMFILE under fd exhaustion
/// would otherwise spin the core at full speed); any success resets the
/// ladder.
fn serve_streams<I>(incoming: I, store: Arc<Store>, config: ValidatedConfig) -> !
where
    I: Iterator<Item = std::io::Result<std::net::TcpStream>>,
{
    let mut consecutive_errors: u32 = 0;
    for stream in incoming {
        let mut stream = match stream {
            Ok(stream) => {
                consecutive_errors = 0;
                stream
            }
            Err(err) => {
                consecutive_errors += 1;
                let delay = accept_backoff(consecutive_errors);
                tracing::warn!(
                    target: "custos::dashboard",
                    error = %err,
                    backoff_ms = delay.as_millis() as u64,
                    "accept failed; backing off"
                );
                std::thread::sleep(delay);
                continue;
            }
        };
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
        if let Err(err) = handle(&mut stream, &store, &config) {
            tracing::warn!(target: "custos::dashboard", error = %err, "request failed");
        }
    }
    unreachable!("listener closed")
}

/// Backoff before the `consecutive_errors`-th retry: doubles from 10ms and
/// caps at 1s (C30). A persistent accept failure idles here instead of
/// spinning; the counter resets to zero on any successful accept.
fn accept_backoff(consecutive_errors: u32) -> std::time::Duration {
    let shift = consecutive_errors.saturating_sub(1).min(7);
    let ms = 10u64.saturating_mul(1u64 << shift).min(1_000);
    std::time::Duration::from_millis(ms)
}

/// Whole-request deadline (C23). The per-read timeout bounds silence, not
/// total time: a slow-drip client that sends one byte just under the read
/// timeout resets the window on every fragment and would otherwise hold the
/// single-threaded accept loop hostage forever. Idle peers still exit via
/// the read timeout; this bounds the drip case.
const REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Handles one HTTP request with a read-only JSON or HTML response.
fn handle(
    stream: &mut std::net::TcpStream,
    store: &Store,
    config: &ValidatedConfig,
) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let mut raw = Vec::new();
    // Read until the request-head terminator or the buffer cap; TCP may
    // deliver the request in fragments, so accumulate rather than read once.
    let started = std::time::Instant::now();
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") || raw.len() > 64 * 1024 {
            break;
        }
        if started.elapsed() >= REQUEST_DEADLINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request did not complete within the whole-request deadline",
            ));
        }
    }
    let request = String::from_utf8_lossy(&raw).to_string();
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();

    let (status, body, content_type) = route(&path, store, config);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Write)
}

fn route(path: &str, store: &Store, config: &ValidatedConfig) -> (String, String, &'static str) {
    match path {
        "/" => (
            "200 OK".to_owned(),
            html_shell(),
            "text/html; charset=utf-8",
        ),
        "/api/health" => json_response(health_payload(store)),
        "/api/runs" => json_response(runs_payload(store)),
        "/api/timeline" => json_response(runs_payload(store)),
        "/api/resources" => json_response(resources_payload(config)),
        "/api/graph" => json_response(graph_payload(config)),
        "/api/permissions" => json_response(permissions_payload(config)),
        "/api/notifications" => json_response(notifications_payload(store)),
        "/api/config" => {
            // Read-only configuration summary: the dashboard never mutates.
            match store.counts() {
                Ok(counts) => {
                    let summary = serde_json::json!({
                        "runs": counts.runs,
                        "attempts": counts.attempts,
                        "verifications": counts.verifications,
                        "audit_events": counts.audit_events,
                        "read_only": true,
                    });
                    json_response(Ok::<_, String>(summary))
                }
                Err(err) => json_response(Err::<serde_json::Value, _>(err.to_string())),
            }
        }
        _ => (
            "404 Not Found".to_owned(),
            r#"{"error":"not found"}"#.to_owned(),
            "application/json; charset=utf-8",
        ),
    }
}

fn health_payload(store: &Store) -> Result<serde_json::Value, String> {
    let rows = store.latest_health().map_err(|e| e.to_string())?;
    let items: Vec<_> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "resource_id": r.resource_id,
                "probe_name": r.probe_name,
                "succeeded": r.succeeded,
                "observation": r.observation,
                "collected_at_unix_ms": r.collected_at_unix_ms,
            })
        })
        .collect();
    Ok(serde_json::json!({ "resources": items }))
}

fn runs_payload(store: &Store) -> Result<serde_json::Value, String> {
    let rows = store.list_runs(100).map_err(|e| e.to_string())?;
    let items: Vec<_> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "resource_id": r.resource_id,
                "status": r.status,
                "trigger": r.trigger,
                "started_at_unix_ms": r.started_at_unix_ms,
                "updated_at_unix_ms": r.updated_at_unix_ms,
            })
        })
        .collect();
    Ok(serde_json::json!({ "runs": items }))
}

fn resources_payload(config: &ValidatedConfig) -> Result<serde_json::Value, String> {
    let items: Vec<_> = config
        .resources
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "type": r.resource_type,
                "display_name": r.display_name,
                "enabled": r.enabled,
                "plans": r.recovery_policy.plans.len(),
                "dependencies": r.dependencies.iter().map(|d| d.target_resource_id.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(serde_json::json!({ "resources": items }))
}

fn graph_payload(config: &ValidatedConfig) -> Result<serde_json::Value, String> {
    let mut nodes: Vec<String> = config
        .resources
        .iter()
        .map(|r| r.id.as_str().to_owned())
        .collect();
    nodes.sort();
    let mut edges: Vec<serde_json::Value> = Vec::new();
    for r in &config.resources {
        for d in &r.dependencies {
            edges.push(serde_json::json!({
                "from": r.id,
                "to": d.target_resource_id,
            }));
        }
    }
    Ok(serde_json::json!({ "nodes": nodes, "edges": edges }))
}

fn permissions_payload(config: &ValidatedConfig) -> Result<serde_json::Value, String> {
    let resources: Vec<_> = config
        .resources
        .iter()
        .map(|r| {
            serde_json::json!({
                "resource_id": r.id,
                "allow_high_risk": r.recovery_policy.allow_high_risk,
                "plans": r.recovery_policy.plans.iter().map(|p| serde_json::json!({
                    "plan_id": p.plan_id,
                    "risk": format!("{:?}", p.risk_level),
                    "idempotent": p.idempotent,
                    "max_attempts": p.max_attempts,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "default_allow_high_risk": config.permissions.default_allow_high_risk,
        "resources": resources,
    }))
}

fn notifications_payload(store: &Store) -> Result<serde_json::Value, String> {
    let rows = store.list_notifications(100).map_err(|e| e.to_string())?;
    let items: Vec<_> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "channel": r.channel,
                "state": r.state,
                "event_type": r.event.event_type,
                "resource_id": r.event.resource_id,
                "attempts": r.attempts,
                "created_at_unix_ms": r.created_at_unix_ms,
            })
        })
        .collect();
    Ok(serde_json::json!({ "notifications": items }))
}

fn json_response<T, E>(result: Result<T, E>) -> (String, String, &'static str)
where
    T: serde::Serialize,
    E: std::fmt::Display,
{
    match result {
        Ok(value) => (
            "200 OK".to_owned(),
            serde_json::to_string(&value).unwrap_or_else(|_| r#"{"error":"serialize"}"#.into()),
            "application/json; charset=utf-8",
        ),
        Err(err) => (
            "500 Internal Server Error".to_owned(),
            format!(
                r#"{{"error":{}}}"#,
                serde_json::to_string(&err.to_string()).unwrap()
            ),
            "application/json; charset=utf-8",
        ),
    }
}

fn html_shell() -> String {
    r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Custos Dashboard</title></head>
<body><h1>Custos</h1><p>Read-only dashboard (default disabled).</p>
<ul>
<li><a href="/api/health">health</a></li>
<li><a href="/api/resources">resources</a></li>
<li><a href="/api/graph">dependency graph</a></li>
<li><a href="/api/runs">runs</a></li>
<li><a href="/api/timeline">timeline</a></li>
<li><a href="/api/notifications">notifications</a></li>
<li><a href="/api/config">config check</a></li>
</ul></body></html>"#
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_config_for_test() -> ValidatedConfig {
        use custos_config::load;
        load("version = 1\n").expect("test config")
    }

    #[test]
    fn disabled_config_has_no_listener_path() {
        let config = DashboardConfig::default();
        assert!(!config.is_enabled());
    }

    #[test]
    fn route_unknown_path_is_404() {
        let config = load_config_for_test();
        let (status, _, _) = route("/nope", &dummy_store(), &config);
        assert!(status.contains("404"), "{status}");
    }

    fn dummy_store() -> Store {
        // Open an in-memory store; Store::open requires a path, so use a
        // temp file. This is only for routing tests that don't hit SQL.
        Store::open(":memory:").unwrap()
    }

    #[test]
    fn slow_drip_client_cannot_hold_the_request_open_past_the_deadline() {
        // C23: the per-read timeout bounds silence, not total time. A
        // client that drips one byte every 300ms (well under the 2s read
        // timeout) resets the read window on every fragment and used to
        // hold the single-threaded accept loop hostage forever.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        // Drip forever from a detached thread: the request head never
        // completes, so only a whole-request deadline can end it.
        std::thread::spawn(move || {
            use std::io::Write;
            let mut client = client;
            loop {
                if client.write_all(b"x").is_err() {
                    return;
                }
                let _ = client.flush();
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        });
        let store = dummy_store();
        let config = load_config_for_test();
        let worker = std::thread::spawn(move || handle(&mut server, &store, &config));
        for _ in 0..40 {
            if worker.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        assert!(
            worker.is_finished(),
            "handle still blocked after 20s: a slow drip holds the accept loop"
        );
        let err = worker
            .join()
            .expect("handle thread")
            .expect_err("handle must fail past the deadline");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
    }

    #[test]
    fn accept_backoff_doubles_and_caps() {
        use std::time::Duration;
        assert_eq!(accept_backoff(1), Duration::from_millis(10));
        assert_eq!(accept_backoff(2), Duration::from_millis(20));
        assert_eq!(accept_backoff(3), Duration::from_millis(40));
        assert_eq!(accept_backoff(7), Duration::from_millis(640));
        assert_eq!(accept_backoff(8), Duration::from_millis(1_000));
        assert_eq!(accept_backoff(100), Duration::from_millis(1_000));
    }

    #[test]
    fn persistent_accept_errors_back_off_instead_of_spinning() {
        // C30: the accept loop used to `continue` on errors with no
        // backoff — a persistent failure (EMFILE) busy-spun the core.
        // Three consecutive errors must cost at least the scheduled
        // 10+20+40ms of backoff.
        use std::time::Duration;
        let config = load_config_for_test();
        let store = std::sync::Arc::new(dummy_store());
        // The listener itself is irrelevant here; bind one so the test
        // documents the real shape (a bound server loop) without needing
        // a live client.
        let _listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let errors: Vec<std::io::Result<std::net::TcpStream>> = vec![
            Err(std::io::Error::other("e1")),
            Err(std::io::Error::other("e2")),
            Err(std::io::Error::other("e3")),
        ];
        let started = std::time::Instant::now();
        // The loop hits `unreachable!("listener closed")` when the source
        // dries up: the thread panics, which is exactly the end we want.
        let worker = std::thread::spawn(move || serve_streams(errors.into_iter(), store, config));
        let _ = worker.join();
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(70),
            "three accept errors took only {elapsed:?} — the loop busy-spins"
        );
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }
}

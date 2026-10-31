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
use custos_store::Store;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

/// One dashboard server bound to the configured listen address. The server is
/// created only when the dashboard is enabled; otherwise no listener exists.
pub struct DashboardServer {
    listener: TcpListener,
    store: Arc<Store>,
}

impl DashboardServer {
    /// Binds the listener. Call only when `config.is_enabled()`.
    pub fn bind(config: &DashboardConfig, store: Arc<Store>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(config.listen_addr())?;
        Ok(Self { listener, store })
    }

    /// The bound local address (useful for logging the actual port).
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Runs the accept loop in the current thread until the listener is
    /// closed. A per-request failure never kills the server: it logs and
    /// continues, keeping the core engine unaffected.
    pub fn serve(self) -> ! {
        for stream in self.listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(_) => continue,
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
            if let Err(err) = handle(&mut stream, &self.store) {
                tracing::warn!(target: "custos::dashboard", error = %err, "request failed");
            }
        }
        unreachable!("listener closed")
    }
}

/// Handles one HTTP request with a read-only JSON or HTML response.
fn handle(stream: &mut std::net::TcpStream, store: &Store) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let mut raw = Vec::new();
    // Read until the request-head terminator or the buffer cap; TCP may
    // deliver the request in fragments, so accumulate rather than read once.
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") || raw.len() > 64 * 1024 {
            break;
        }
    }
    let request = String::from_utf8_lossy(&raw).to_string();
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();

    let (status, body, content_type) = route(&path, store);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Write)
}

fn route(path: &str, store: &Store) -> (String, String, &'static str) {
    match path {
        "/" => (
            "200 OK".to_owned(),
            html_shell(),
            "text/html; charset=utf-8",
        ),
        "/api/health" => json_response(health_payload(store)),
        "/api/runs" => json_response(runs_payload(store)),
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
<li><a href="/api/runs">runs</a></li>
<li><a href="/api/notifications">notifications</a></li>
<li><a href="/api/config">config check</a></li>
</ul></body></html>"#
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_config_has_no_listener_path() {
        let config = DashboardConfig::default();
        assert!(!config.is_enabled());
    }

    #[test]
    fn route_unknown_path_is_404() {
        let (status, _, _) = route("/nope", &dummy_store());
        assert!(status.contains("404"), "{status}");
    }

    fn dummy_store() -> Store {
        // Open an in-memory store; Store::open requires a path, so use a
        // temp file. This is only for routing tests that don't hit SQL.
        Store::open(":memory:").unwrap()
    }
}

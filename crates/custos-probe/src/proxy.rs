//! Proxy connectivity and authentication probe (todo.md P3 216; survey §26
//! 代理诊断: 区分认证失败/连接失败/目标异常 — 407 ≠ 宕机).
//!
//! Sends an absolute-form HTTP GET through the proxy and classifies the
//! proxy's answer:
//! * `2xx/3xx` → Healthy (proxy answered and reached the target);
//! * `407` (or proxy auth challenge) → **Degraded** — the proxy is UP but
//!   authentication failed; restarting it would be wrong;
//! * `5xx` from the proxy (gateway errors) → Unhealthy (target-side or
//!   proxy-side failure, a real fault to act on);
//! * proxy connection refused/timeout/DNS → Unreachable.
//!
//! Pure std TCP + HTTP/1.1; no extra dependencies.

use crate::config::{optional_str, required_str, u64_or};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_TARGET: &str = "http://www.gstatic.com/generate_204";

/// One proxy diagnostic: GET `target` through `proxy`.
#[derive(Debug, Clone)]
pub struct ProxyProbe {
    /// Proxy endpoint as `host:port`.
    pub proxy: String,
    /// Absolute URL to fetch through the proxy.
    pub target: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub timeout: Duration,
}

/// Builds from `[resources.probe]` (`proxy` required).
pub fn from_config(config: &serde_json::Value) -> Result<ProxyProbe, String> {
    let proxy = required_str(config, "proxy")
        .ok_or("`probe.proxy` is required for proxy resources (host:port)")?;
    let timeout = Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS));
    Ok(ProxyProbe {
        proxy,
        target: config["target"]
            .as_str()
            .unwrap_or(DEFAULT_TARGET)
            .to_owned(),
        username: optional_str(config, "username"),
        password: optional_str(config, "password"),
        timeout,
    })
}

/// The normalized proxy verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyVerdict {
    Healthy,
    /// The proxy is up but rejected our credentials.
    AuthRequired,
    /// The proxy answered but could not serve the target.
    TargetFailed,
    /// The proxy itself could not be reached.
    Unreachable,
}

/// Classifies a proxy status line (pure, fixture-tested).
pub fn classify_status(status: u16) -> ProxyVerdict {
    match status {
        200..=399 => ProxyVerdict::Healthy,
        401 | 407 => ProxyVerdict::AuthRequired,
        500..=599 => ProxyVerdict::TargetFailed,
        // 4xx from the proxy for our request is a proxy-side answer about
        // the target being unreachable through it.
        _ => ProxyVerdict::TargetFailed,
    }
}

/// Builds the absolute-form request (pure).
pub fn build_request(target: &str, host_header: &str, auth: Option<&str>) -> String {
    let mut request = format!("GET {target} HTTP/1.1\r\nHost: {host_header}\r\n");
    if let Some(auth) = auth {
        request.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    request
}

/// base64 (standard alphabet) for Basic auth.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        out.push(TABLE[(b[0] >> 2) as usize] as char);
        out.push(TABLE[((b[0] & 0x03) << 4 | b[1] >> 4) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((b[1] & 0x0f) << 2 | b[2] >> 6) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b[2] & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Parses the status code out of an HTTP response head (pure).
pub fn parse_status(head: &str) -> Option<u16> {
    let first = head.lines().next()?;
    let code = first.split_whitespace().nth(1)?;
    code.parse().ok()
}

impl Probe for ProxyProbe {
    fn name(&self) -> &'static str {
        "proxy"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let (host_header, target_path) = split_target(&self.target);
        let auth = self.username.as_ref().map(|user| {
            base64_encode(format!("{user}:{}", self.password.as_deref().unwrap_or("")).as_bytes())
        });
        let request = build_request(target_path, host_header, auth.as_deref());

        let answer = (|| -> Result<String, std::io::Error> {
            // proxy is `host:port`; a bare host gets the default 8080.
            let endpoint = if self.proxy.contains(':') {
                self.proxy.clone()
            } else {
                format!("{}:{}", self.proxy, default_port(&self.proxy))
            };
            let addr = endpoint.to_socket_addrs_first()?;
            let mut stream = TcpStream::connect_timeout(&addr, self.timeout)?;
            stream.set_read_timeout(Some(self.timeout))?;
            stream.set_write_timeout(Some(self.timeout))?;
            stream.write_all(request.as_bytes())?;
            let mut head = Vec::new();
            let mut buf = [0u8; 4096];
            // Read until the blank line (headers complete) or cap.
            loop {
                let n = stream.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&head);
                if text.contains("\r\n\r\n") || head.len() > 64 * 1024 {
                    break;
                }
            }
            Ok(String::from_utf8_lossy(&head).to_string())
        })();

        match answer {
            Ok(head) => match parse_status(&head) {
                Some(status) => match classify_status(status) {
                    ProxyVerdict::Healthy => report(
                        true,
                        HealthStatus::Healthy,
                        format!("proxy reached target, HTTP {status}"),
                    ),
                    ProxyVerdict::AuthRequired => report(
                        false,
                        HealthStatus::Degraded,
                        format!(
                            "proxy is up but authentication failed (HTTP {status}); \
                             check credentials — not downtime"
                        ),
                    ),
                    ProxyVerdict::TargetFailed => report(
                        false,
                        HealthStatus::Unhealthy,
                        format!("proxy answered HTTP {status}: target not served through proxy"),
                    ),
                    ProxyVerdict::Unreachable => report(
                        false,
                        HealthStatus::Unreachable,
                        format!("proxy answered HTTP {status}"),
                    ),
                },
                None => report(
                    false,
                    HealthStatus::Unreachable,
                    "proxy closed the connection without an HTTP answer".into(),
                ),
            },
            Err(err) => match err.kind() {
                std::io::ErrorKind::ConnectionRefused => report(
                    false,
                    HealthStatus::Unreachable,
                    format!("proxy connection refused: {err}"),
                ),
                _ if err.to_string().contains("resolve") || err.to_string().contains("dns") => {
                    report(
                        false,
                        HealthStatus::Unreachable,
                        format!("proxy dns: {err}"),
                    )
                }
                _ => report(
                    false,
                    HealthStatus::Unreachable,
                    format!("proxy unreachable: {err}"),
                ),
            },
        }
    }
}

fn split_target(target: &str) -> (&str, &str) {
    // `http://host:port/path` → host:port for Host header, path for request.
    let after_scheme = target
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(target);
    match after_scheme.find('/') {
        Some(idx) => (&after_scheme[..idx], &after_scheme[idx..]),
        None => (after_scheme, "/"),
    }
}

fn default_port(proxy: &str) -> u16 {
    proxy
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080)
}

trait ToSocketAddrsFirst {
    fn to_socket_addrs_first(&self) -> std::io::Result<std::net::SocketAddr>;
}

impl ToSocketAddrsFirst for str {
    fn to_socket_addrs_first(&self) -> std::io::Result<std::net::SocketAddr> {
        use std::net::ToSocketAddrs;
        self.to_socket_addrs()?.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "proxy host has no address")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn resource() -> Resource {
        Resource::new("proxy", "proxy")
    }

    fn from_json(value: serde_json::Value) -> ProxyProbe {
        from_config(&value).unwrap()
    }

    #[test]
    fn from_config_requires_proxy() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"proxy": "127.0.0.1:7890"})).is_ok());
        let probe = from_json(json!({"proxy": "127.0.0.1:7890"}));
        assert_eq!(probe.timeout, Duration::from_millis(5_000));
        assert_eq!(probe.target, DEFAULT_TARGET);
    }

    #[test]
    fn status_classification_407_is_auth_not_downtime() {
        assert_eq!(classify_status(200), ProxyVerdict::Healthy);
        assert_eq!(classify_status(302), ProxyVerdict::Healthy);
        assert_eq!(classify_status(407), ProxyVerdict::AuthRequired);
        assert_eq!(classify_status(401), ProxyVerdict::AuthRequired);
        assert_eq!(classify_status(502), ProxyVerdict::TargetFailed);
        assert_eq!(classify_status(403), ProxyVerdict::TargetFailed);
    }

    #[test]
    fn request_is_absolute_form_with_auth() {
        let request = build_request(
            "http://example.com/health",
            "example.com",
            Some("dXNlcjpwYXNz"),
        );
        assert!(
            request.starts_with("GET http://example.com/health HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("Host: example.com"));
        assert!(request.contains("Proxy-Authorization: Basic dXNlcjpwYXNz"));
        let anonymous = build_request("http://example.com/", "example.com", None);
        assert!(!anonymous.contains("Proxy-Authorization"));
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }

    #[test]
    fn status_line_parse() {
        assert_eq!(
            parse_status("HTTP/1.1 407 Proxy Authentication Required\r\n"),
            Some(407)
        );
        assert_eq!(parse_status("HTTP/1.0 502 Bad Gateway\r\n"), Some(502));
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn target_split_handles_paths_and_bare_hosts() {
        assert_eq!(
            split_target("http://example.com/health"),
            ("example.com", "/health")
        );
        assert_eq!(split_target("http://example.com"), ("example.com", "/"));
    }

    #[test]
    fn refused_proxy_is_unreachable() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };
        let probe = from_json(json!({"proxy": format!("127.0.0.1:{port}"), "timeout_ms": 500}));
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Unreachable,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("refused"),
            "{}",
            report.observation
        );
    }

    /// A fake proxy answering with the given status line.
    fn fake_proxy(status_line: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let status_line = status_line.to_owned();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = "";
                let response = format!(
                    "{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        addr.to_string()
    }

    #[test]
    fn proxy_407_is_degraded_not_unreachable() {
        let addr = fake_proxy("HTTP/1.1 407 Proxy Authentication Required");
        let probe = from_json(json!({"proxy": addr, "timeout_ms": 1000}));
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Degraded,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("not downtime"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn proxy_200_is_healthy() {
        let addr = fake_proxy("HTTP/1.1 200 Connection established");
        let probe = from_json(json!({"proxy": addr, "timeout_ms": 1000}));
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
    }

    #[test]
    fn proxy_502_is_unhealthy_target_failure() {
        let addr = fake_proxy("HTTP/1.1 502 Bad Gateway");
        let probe = from_json(json!({"proxy": addr, "timeout_ms": 1000}));
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Unhealthy,
            "{}",
            report.observation
        );
    }
}

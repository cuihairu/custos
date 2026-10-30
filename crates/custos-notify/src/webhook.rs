//! Webhook channel (docs/guide/notification.md §8.2) — the generic remote
//! channel. POSTs each event as JSON to a configured URL with a request
//! timeout, configurable auth headers, optional HMAC-SHA256 request signing
//! and TLS verification control, and retries transient transport/5xx
//! failures with exponential backoff.
//!
//! Error classification distinguishes the failure kinds the doc asks for:
//! transport problems (connect/DNS/timeout) → `Transport` (retryable),
//! HTTP 5xx → `Http` (retryable), HTTP 4xx → `Rejected` (terminal), bad
//! URL → `Config` (terminal). Secrets are never echoed in errors.

use crate::channel::{Channel, ChannelError, ChannelErrorKind};
use crate::local_log::format_event;
use custos_config::{SecretString, WebhookConfig};
use custos_model::NotificationEvent;
use std::time::Duration;

/// Signature header carrying the HMAC of the request body.
pub const SIGNATURE_HEADER: &str = "X-Custos-Signature";
/// Event-id header so the receiver can deduplicate retries (§8.6).
pub const EVENT_ID_HEADER: &str = "X-Custos-Event-Id";

/// One webhook delivery sink.
pub struct WebhookChannel {
    id: String,
    config: WebhookConfig,
    agent: ureq::Agent,
}

impl WebhookChannel {
    /// Builds the HTTP agent from the validated config. `tls_verify=false`
    /// disables server-certificate verification (self-signed test hooks).
    /// The SSRF policy is enforced at construction and again before every
    /// send, and redirects are disabled when the policy blocks them.
    pub fn new(id: impl Into<String>, config: WebhookConfig) -> Result<Self, ChannelError> {
        // Fail fast on a policy-violating target at construction time.
        crate::webhook_ssrf::validate_target(&config.url, &config.ssrf).map_err(|block| {
            ChannelError::new(
                ChannelErrorKind::Config,
                format!(
                    "webhook target blocked by SSRF policy: {}",
                    block.describe()
                ),
            )
        })?;
        let tls = ureq::tls::TlsConfig::builder()
            .disable_verification(!config.tls_verify)
            .build();
        let timeout = Duration::from_millis(config.timeout_ms);
        let mut builder = ureq::config::Config::builder()
            .tls_config(tls)
            .timeout_global(Some(timeout))
            .timeout_connect(Some(timeout));
        if config.ssrf.block_redirects {
            builder = builder.max_redirects(0);
        }
        let agent = ureq::Agent::new_with_config(builder.build());
        Ok(Self {
            id: id.into(),
            config,
            agent,
        })
    }

    /// One attempt: re-check the SSRF policy against the target (fresh DNS
    /// resolution), build the signed JSON body, and POST it.
    fn send_once(&self, event: &NotificationEvent, body: &str) -> Result<(), ChannelError> {
        crate::webhook_ssrf::validate_target(&self.config.url, &self.config.ssrf).map_err(
            |block| {
                ChannelError::new(
                    ChannelErrorKind::Config,
                    format!(
                        "webhook target blocked by SSRF policy: {}",
                        block.describe()
                    ),
                )
            },
        )?;
        let mut request = self
            .agent
            .post(&self.config.url)
            .header("Content-Type", "application/json")
            .header(EVENT_ID_HEADER, event.event_id.as_str());
        for (key, value) in &self.config.headers {
            request = request.header(key.as_str(), value.as_str());
        }
        if let Some(secret) = &self.config.hmac_secret {
            let signature = sign_body(secret, body.as_bytes());
            request = request.header(SIGNATURE_HEADER, signature.as_str());
        }
        match request.send(body.to_owned()) {
            Ok(response) => {
                let status = response.status().as_u16();
                if (200..300).contains(&status) {
                    Ok(())
                } else if (300..400).contains(&status) {
                    // A redirect surfaced despite the block: the target
                    // policy was violated, so this is terminal.
                    Err(ChannelError::new(
                        ChannelErrorKind::Rejected,
                        format!("webhook redirected to HTTP {status} (blocked by SSRF policy)"),
                    ))
                } else {
                    Err(ChannelError::new(
                        ChannelErrorKind::Http,
                        format!("webhook failed with HTTP {status}"),
                    ))
                }
            }
            Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) => {
                Err(ChannelError::new(
                    ChannelErrorKind::Rejected,
                    format!("webhook rejected with HTTP {code}"),
                ))
            }
            Err(ureq::Error::StatusCode(code)) => Err(ChannelError::new(
                ChannelErrorKind::Http,
                format!("webhook failed with HTTP {code}"),
            )),
            Err(ureq::Error::BadUri(_)) => Err(ChannelError::new(
                ChannelErrorKind::Config,
                "webhook url is not a valid URI".to_owned(),
            )),
            Err(other) => Err(ChannelError::new(
                ChannelErrorKind::Transport,
                format!("webhook transport error: {other}"),
            )),
        }
    }

    /// Sends with `retries` retries of transient failures, doubling the
    /// backoff each time. Terminal errors (`Rejected`, `Config`) return
    /// immediately.
    fn deliver_with_retry(&self, event: &NotificationEvent) -> Result<(), ChannelError> {
        let body = format_event(event);
        let mut retried = 0u32;
        let mut delay = Duration::from_millis(self.config.backoff_ms);
        loop {
            match self.send_once(event, &body) {
                Ok(()) => return Ok(()),
                Err(e)
                    if matches!(
                        e.kind,
                        ChannelErrorKind::Rejected | ChannelErrorKind::Config
                    ) =>
                {
                    return Err(e);
                }
                Err(e) => {
                    if retried >= self.config.retries {
                        return Err(e);
                    }
                    retried += 1;
                    std::thread::sleep(delay);
                    delay = delay.saturating_mul(2);
                }
            }
        }
    }
}

impl Channel for WebhookChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn deliver(&self, event: &NotificationEvent) -> Result<(), ChannelError> {
        self.deliver_with_retry(event)
    }
}

/// HMAC-SHA256 of the body, hex-encoded with a `sha256=` algorithm prefix so
/// the receiver selects the right verifier. The secret comes from config and
/// is never logged (docs/guide/notification.md §8.7).
pub fn sign_body(secret: &SecretString, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(secret.expose().as_bytes()).expect("hmac accepts any key");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(7 + digest.len() * 2);
    hex.push_str("sha256=");
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_log::format_event;
    use custos_config::SsrfPolicy;
    use custos_model::{EventId, EventType, NotificationEvent, Severity};
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc, Mutex};

    fn event() -> NotificationEvent {
        NotificationEvent::new(
            EventId::new("evt-9"),
            EventType::ResourceUnhealthy,
            Severity::Critical,
            "db down",
            1_700_000_000_000,
        )
    }

    fn config(url: impl Into<String>) -> WebhookConfig {
        // The transport tests talk to a loopback HTTP server, so the SSRF
        // policy must permit loopback there; the SSRF rules themselves are
        // covered by webhook_ssrf tests and the blocked-target test below.
        let mut ssrf = SsrfPolicy::default_blocking();
        ssrf.block_loopback = false;
        WebhookConfig {
            url: url.into(),
            timeout_ms: 2000,
            retries: 2,
            backoff_ms: 1,
            headers: Default::default(),
            hmac_secret: None,
            tls_verify: true,
            ssrf,
        }
    }

    /// Spawns a single-shot HTTP responder on an ephemeral port, capturing
    /// the raw request, and replies with the given status.
    fn responder(status: u16) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = crate::test_http::read_full_request(&mut stream);
            cap.lock().unwrap().extend_from_slice(&request);
            let response =
                format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (addr, captured)
    }

    /// Spawns a responder that answers `count` requests in order.
    fn responder_seq(statuses: Vec<u16>) -> (SocketAddr, Arc<Mutex<Vec<Vec<u8>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        std::thread::spawn(move || {
            for status in statuses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = crate::test_http::read_full_request(&mut stream);
                cap.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (addr, captured)
    }

    #[test]
    fn sign_body_matches_known_hmac_sha256_vector() {
        let secret = SecretString::new("key");
        let digest = sign_body(&secret, b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            digest,
            "sha256=f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn sign_body_differs_per_secret_and_payload() {
        let a = sign_body(&SecretString::new("k1"), b"body");
        let b = sign_body(&SecretString::new("k2"), b"body");
        let c = sign_body(&SecretString::new("k1"), b"other");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("sha256="));
    }

    #[test]
    fn delivers_and_posts_json_with_event_id_header() {
        let (addr, captured) = responder(200);
        let channel = WebhookChannel::new("wh", config(format!("http://{addr}/hook"))).unwrap();
        channel.deliver(&event()).unwrap();

        // ureq lowercases header names; compare case-insensitively.
        let request = String::from_utf8_lossy(&captured.lock().unwrap().clone()).to_lowercase();
        assert!(request.starts_with("post /hook"), "{request}");
        assert!(request.contains("x-custos-event-id: evt-9"), "{request}");
        assert!(
            request.contains("content-type: application/json"),
            "{request}"
        );
        // Body is the event JSON.
        let body = format_event(&event()).to_lowercase();
        assert!(request.contains(&body), "{request}");
    }

    #[test]
    fn signs_when_secret_configured() {
        let (addr, captured) = responder(200);
        let mut cfg = config(format!("http://{addr}/hook"));
        cfg.hmac_secret = Some(SecretString::new("top-secret"));
        let channel = WebhookChannel::new("wh", cfg).unwrap();
        channel.deliver(&event()).unwrap();

        let request = String::from_utf8_lossy(&captured.lock().unwrap().clone()).to_lowercase();
        assert!(request.contains("x-custos-signature: sha256="), "{request}");
    }

    #[test]
    fn omits_signature_when_no_secret() {
        let (addr, captured) = responder(200);
        let channel = WebhookChannel::new("wh", config(format!("http://{addr}/hook"))).unwrap();
        channel.deliver(&event()).unwrap();
        let request = String::from_utf8_lossy(&captured.lock().unwrap().clone()).to_lowercase();
        assert!(!request.contains("x-custos-signature"), "{request}");
    }

    #[test]
    fn rejects_4xx_without_retrying() {
        let (addr, _) = responder_seq(vec![404]);
        let channel = WebhookChannel::new("wh", config(format!("http://{addr}/hook"))).unwrap();
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Rejected);
        assert!(err.to_string().contains("404"), "{err}");
    }

    #[test]
    fn retries_transient_then_succeeds() {
        // First two attempts fail with 500, third succeeds.
        let (addr, captured) = responder_seq(vec![500, 500, 200]);
        let mut cfg = config(format!("http://{addr}/hook"));
        cfg.retries = 2;
        cfg.backoff_ms = 1;
        let channel = WebhookChannel::new("wh", cfg).unwrap();
        channel.deliver(&event()).unwrap();
        assert_eq!(captured.lock().unwrap().len(), 3);
    }

    #[test]
    fn gives_up_after_retries_on_transient() {
        let (addr, captured) = responder_seq(vec![500, 500, 500]);
        let mut cfg = config(format!("http://{addr}/hook"));
        cfg.retries = 1;
        cfg.backoff_ms = 1;
        let channel = WebhookChannel::new("wh", cfg).unwrap();
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Http);
        // First attempt + one retry.
        assert_eq!(captured.lock().unwrap().len(), 2);
    }

    #[test]
    fn connection_refused_is_transport() {
        // Bind then drop to get a refused port.
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            drop(listener);
            addr
        };
        let mut cfg = config(format!("http://{addr}/hook"));
        cfg.retries = 0;
        let channel = WebhookChannel::new("wh", cfg).unwrap();
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Transport);
    }

    #[test]
    fn construction_rejects_ssrf_blocked_target() {
        // Default policy blocks loopback; construction must fail fast.
        let cfg = WebhookConfig {
            url: "http://127.0.0.1:8080/hook".into(),
            timeout_ms: 2000,
            retries: 0,
            backoff_ms: 1,
            headers: Default::default(),
            hmac_secret: None,
            tls_verify: true,
            ssrf: SsrfPolicy::default_blocking(),
        };
        let err = match WebhookChannel::new("wh", cfg) {
            Err(e) => e,
            Ok(_) => panic!("expected construction to be rejected"),
        };
        assert_eq!(err.kind, ChannelErrorKind::Config);
        assert!(err.to_string().contains("SSRF"), "{err}");
    }

    #[test]
    fn allowlisted_target_bypasses_ssrf() {
        // An allowlisted host name is accepted even though it resolves to
        // loopback, and the channel then fails on connect (refused) rather
        // than on the SSRF policy.
        let mut ssrf = SsrfPolicy::default_blocking();
        ssrf.allowlist = vec!["internal.example.com".into()];
        let cfg = WebhookConfig {
            url: "http://internal.example.com:9/hook".into(),
            timeout_ms: 500,
            retries: 0,
            backoff_ms: 1,
            headers: Default::default(),
            hmac_secret: None,
            tls_verify: true,
            ssrf,
        };
        let channel = WebhookChannel::new("wh", cfg).unwrap();
        // Port 9 (discard) is refused; allowlist bypasses the block, so the
        // failure is a transport error, not an SSRF block.
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Transport);
    }
}

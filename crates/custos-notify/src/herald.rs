//! Herald channel (github.com/cuihairu/herald) — a first-class integration
//! with the user's notification orchestration service. POSTs each event to
//! the app dispatch endpoint `/api/v1/apps/{app}/dispatch` with the app
//! token (`X-API-Key` or `Authorization: Bearer`).
//!
//! SSRF protection mirrors the webhook channel (docs/guide/notification.md
//! §8.2): `base_url` is validated against the configured `SsrfPolicy` at
//! construction and re-checked before every send; redirects are disabled
//! when the policy blocks them.
//!
//! Mapping: Custos severity → herald urgency (`info→normal`,
//! `warning→urgent`, `critical→critical`); fault-type events carry herald's
//! `state = "down"` and `recovery.succeeded` carries `state = "ok"`, so
//! herald-side dedup collapses repeats of the same fault and a state flip
//! resends. Our stable event id is sent as `event_id` (herald echoes it on
//! delivery-result callbacks) and `{event_type}:{resource}` as `dedup_key`.
//!
//! The dispatch response is an acceptance result: `{"code":0}` means herald
//! accepted the notification (even when its own dedup suppressed it or it
//! refused individual audiences); herald only reports `code:422` when every
//! channel delivery failed.

use crate::channel::{Channel, ChannelError, ChannelErrorKind};
use custos_config::{HeraldAuth, HeraldConfig};
use custos_model::{EventType, NotificationEvent, Severity};
use serde_json::json;
use std::time::Duration;

/// One herald dispatch sink.
pub struct HeraldChannel {
    id: String,
    config: HeraldConfig,
    agent: ureq::Agent,
}

impl HeraldChannel {
    /// Builds the HTTP agent from the validated config. The SSRF policy is
    /// enforced at construction and again before every send (fresh DNS
    /// resolution, same as the webhook channel), and redirects are disabled
    /// when the policy blocks them.
    pub fn new(id: impl Into<String>, config: HeraldConfig) -> Result<Self, ChannelError> {
        // Fail fast on a policy-violating target at construction time.
        crate::webhook_ssrf::validate_target(&config.base_url, &config.ssrf).map_err(|block| {
            ChannelError::new(
                ChannelErrorKind::Config,
                format!("herald target blocked by SSRF policy: {}", block.describe()),
            )
        })?;
        let timeout = Duration::from_millis(config.timeout_ms);
        let mut builder = ureq::config::Config::builder()
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

    /// The dispatch endpoint URL for the configured app.
    fn endpoint(&self) -> String {
        format!(
            "{}/api/v1/apps/{}/dispatch",
            self.config.base_url, self.config.app
        )
    }

    /// One attempt: re-check the SSRF policy against the target, then POST
    /// the pre-built dispatch payload.
    fn send_once(&self, payload: &str) -> Result<(), ChannelError> {
        crate::webhook_ssrf::validate_target(&self.config.base_url, &self.config.ssrf).map_err(
            |block| {
                ChannelError::new(
                    ChannelErrorKind::Config,
                    format!("herald target blocked by SSRF policy: {}", block.describe()),
                )
            },
        )?;
        let mut request = self
            .agent
            .post(self.endpoint())
            .header("Content-Type", "application/json");
        if let Some(token) = &self.config.token {
            request = match self.config.auth {
                HeraldAuth::ApiKey => request.header("X-API-Key", token.expose()),
                HeraldAuth::Bearer => request.header(
                    "Authorization",
                    format!("Bearer {}", token.expose()).as_str(),
                ),
            };
        }
        match request.send(payload.to_owned()) {
            Ok(response) => self.classify_response(response),
            Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) => Err(
                ChannelError::new(ChannelErrorKind::Rejected, format!("herald HTTP {code}")),
            ),
            Err(ureq::Error::StatusCode(code)) => Err(ChannelError::new(
                ChannelErrorKind::Http,
                format!("herald HTTP {code}"),
            )),
            Err(other) => Err(ChannelError::new(
                ChannelErrorKind::Transport,
                format!("herald transport error: {other}"),
            )),
        }
    }

    /// A 2xx response must carry herald's `{"code":0}` acceptance envelope;
    /// anything else (herald writes `code` = HTTP status on error) is a
    /// failure. The 2xx body cases are distinguished (§8.2): an unreadable
    /// body is a transport problem (retryable), a readable body without the
    /// envelope is a contract mismatch (terminal — retrying cannot fix it).
    fn classify_response(
        &self,
        mut response: ureq::http::Response<ureq::Body>,
    ) -> Result<(), ChannelError> {
        let status = response.status().as_u16();
        let code = match response.body_mut().read_to_string() {
            Err(err) => {
                if (200..300).contains(&status) {
                    return Err(ChannelError::new(
                        ChannelErrorKind::Transport,
                        format!("herald HTTP {status} body unreadable: {err}"),
                    ));
                }
                // Non-2xx with an unreadable body: the status still decides.
                None
            }
            Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|value| value.get("code").and_then(|c| c.as_i64())),
        };
        if (200..300).contains(&status) && code == Some(0) {
            return Ok(());
        }
        if (200..300).contains(&status) {
            // 2xx without a readable acceptance envelope: the peer answered
            // at the HTTP level but did not confirm dispatch — retrying an
            // identical request cannot fix that (C4).
            return Err(ChannelError::new(
                ChannelErrorKind::Rejected,
                format!("herald answered HTTP {status} without an acceptance envelope"),
            ));
        }
        let message = format!("herald rejected: HTTP {status}, code {code:?}");
        let kind = match status {
            401 | 403 | 422 => ChannelErrorKind::Rejected,
            _ => ChannelErrorKind::Http,
        };
        Err(ChannelError::new(kind, message))
    }

    /// Sends with `retries` retries of transient failures, doubling the
    /// backoff each time. Terminal errors return immediately.
    fn deliver_with_retry(&self, event: &NotificationEvent) -> Result<(), ChannelError> {
        let payload = dispatch_payload(event, &self.config).to_string();
        let mut retried = 0u32;
        let mut delay = Duration::from_millis(self.config.backoff_ms);
        loop {
            match self.send_once(&payload) {
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

impl Channel for HeraldChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn deliver(&self, event: &NotificationEvent) -> Result<(), ChannelError> {
        self.deliver_with_retry(event)
    }
}

/// Builds the herald dispatch payload (see the herald app API: category is
/// required; urgency/audiences map from config; event_id, dedup_key, state
/// and title/body derive from the event).
pub fn dispatch_payload(event: &NotificationEvent, config: &HeraldConfig) -> serde_json::Value {
    let mut payload = json!({
        "category": config.category,
        "urgency": urgency(event.severity),
        "audiences": config.audiences,
        "event_id": event.event_id.as_str(),
        "dedup_key": dedup_key(event),
        "title": title(event),
        "body": event.summary,
    });
    if let Some(state) = herald_state(event.event_type) {
        payload["state"] = json!(state);
    }
    payload
}

/// Custos severity → herald urgency (herald's `events` endpoint uses the
/// same mapping).
pub fn urgency(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "normal",
        Severity::Warning => "urgent",
        Severity::Critical => "critical",
    }
}

/// Fault-state events drive herald's `state` machine: `down` while faulted,
/// `ok` on verified recovery, absent for progress events (herald collapses
/// repeats of the same state and resends on a flip).
pub fn herald_state(event_type: EventType) -> Option<&'static str> {
    match event_type {
        EventType::ResourceUnhealthy
        | EventType::ResourceDegraded
        | EventType::RecoveryExhausted
        | EventType::RecoveryOutcomeUnknown
        | EventType::PluginUnavailable
        | EventType::ConfigurationInvalid => Some("down"),
        EventType::RecoverySucceeded => Some("ok"),
        EventType::RecoveryStarted
        | EventType::RecoveryActionStarted
        | EventType::RecoveryActionFailed
        | EventType::NotificationDeliveryFailed => None,
    }
}

/// Stable per-fault key: `{event_type}:{resource}` (or `{event_type}:-`
/// when the event has no resource). Herald collapses repeats with the same
/// key + state.
pub fn dedup_key(event: &NotificationEvent) -> String {
    let resource = event
        .resource_id
        .as_ref()
        .map(|r| r.as_str())
        .unwrap_or("-");
    format!("{}:{}", event.event_type.as_str(), resource)
}

/// Concise, greppable title: `[custos] <event_type>: <summary>`, truncated.
pub fn title(event: &NotificationEvent) -> String {
    let mut title = format!("[custos] {}: {}", event.event_type.as_str(), event.summary);
    if title.chars().count() > 120 {
        title = title.chars().take(117).collect::<String>();
        title.push_str("...");
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_model::EventId;
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc, Mutex};

    fn event(event_type: EventType, severity: Severity) -> NotificationEvent {
        NotificationEvent::new(
            EventId::new("evt-77"),
            event_type,
            severity,
            "db down",
            1_000,
        )
        .with_resource_id(custos_model::ResourceId::new("db"))
    }

    fn config() -> HeraldConfig {
        // The transport tests talk to a loopback responder, so the SSRF
        // policy must permit loopback there; the blocked-target behavior is
        // covered by the SSRF tests below.
        let mut ssrf = custos_config::SsrfPolicy::default_blocking();
        ssrf.block_loopback = false;
        HeraldConfig {
            base_url: "http://127.0.0.1:8080".into(),
            app: "custos".into(),
            token: Some(custos_config::SecretString::new("trigger-token")),
            auth: HeraldAuth::ApiKey,
            category: "alerts".into(),
            audiences: vec!["log".into()],
            timeout_ms: 2000,
            retries: 1,
            backoff_ms: 1,
            ssrf,
        }
    }

    #[test]
    fn urgency_maps_from_severity() {
        assert_eq!(urgency(Severity::Info), "normal");
        assert_eq!(urgency(Severity::Warning), "urgent");
        assert_eq!(urgency(Severity::Critical), "critical");
    }

    #[test]
    fn fault_events_are_down_and_recovery_is_ok() {
        assert_eq!(herald_state(EventType::ResourceUnhealthy), Some("down"));
        assert_eq!(herald_state(EventType::RecoveryExhausted), Some("down"));
        assert_eq!(herald_state(EventType::ConfigurationInvalid), Some("down"));
        assert_eq!(herald_state(EventType::RecoverySucceeded), Some("ok"));
        // Progress events carry no state.
        assert_eq!(herald_state(EventType::RecoveryStarted), None);
        assert_eq!(herald_state(EventType::RecoveryActionFailed), None);
    }

    #[test]
    fn dedup_key_is_stable_per_fault_and_resource() {
        assert_eq!(
            dedup_key(&event(EventType::ResourceUnhealthy, Severity::Critical)),
            "resource.unhealthy:db"
        );
        let no_resource = NotificationEvent::new(
            EventId::new("e"),
            EventType::ConfigurationInvalid,
            Severity::Warning,
            "s",
            1_000,
        );
        assert_eq!(dedup_key(&no_resource), "configuration.invalid:-");
    }

    #[test]
    fn payload_carries_id_state_and_audiences() {
        let payload = dispatch_payload(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            &config(),
        );
        assert_eq!(payload["category"], "alerts");
        assert_eq!(payload["urgency"], "critical");
        assert_eq!(payload["audiences"], serde_json::json!(["log"]));
        assert_eq!(payload["event_id"], "evt-77");
        assert_eq!(payload["dedup_key"], "resource.unhealthy:db");
        assert_eq!(payload["state"], "down");
        assert_eq!(payload["title"], "[custos] resource.unhealthy: db down");
        assert_eq!(payload["body"], "db down");

        let recovered = dispatch_payload(
            &event(EventType::RecoverySucceeded, Severity::Info),
            &config(),
        );
        assert_eq!(recovered["state"], "ok");
    }

    /// Single-shot responder echoing a JSON envelope.
    fn responder(status: u16, envelope: &str) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        let envelope = envelope.to_owned();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = crate::test_http::read_full_request(&mut stream);
            cap.lock().unwrap().extend_from_slice(&request);
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{envelope}",
                envelope.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (addr, captured)
    }

    #[test]
    fn delivers_with_api_key_header() {
        let (addr, captured) = responder(200, r#"{"code":0,"message":"ok"}"#);
        let mut cfg = config();
        cfg.base_url = format!("http://{addr}");
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        channel
            .deliver(&event(EventType::ResourceUnhealthy, Severity::Critical))
            .unwrap();

        let request = String::from_utf8_lossy(&captured.lock().unwrap().clone()).to_lowercase();
        assert!(
            request.contains("post /api/v1/apps/custos/dispatch"),
            "{request}"
        );
        assert!(request.contains("x-api-key: trigger-token"), "{request}");
        assert!(request.contains("\"state\":\"down\""), "{request}");
    }

    #[test]
    fn bearer_auth_when_configured() {
        let (addr, captured) = responder(200, r#"{"code":0,"message":"ok"}"#);
        let mut cfg = config();
        cfg.base_url = format!("http://{addr}");
        cfg.auth = HeraldAuth::Bearer;
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        channel
            .deliver(&event(EventType::RecoverySucceeded, Severity::Info))
            .unwrap();
        let request = String::from_utf8_lossy(&captured.lock().unwrap().clone()).to_lowercase();
        assert!(
            request.contains("authorization: bearer trigger-token"),
            "{request}"
        );
        assert!(request.contains("\"state\":\"ok\""), "{request}");
    }

    #[test]
    fn suppressed_still_counts_as_accepted() {
        // Herald's own dedup: suppressed:true, but code 0.
        let (addr, _) = responder(
            200,
            r#"{"code":0,"data":{"notification_id":"n","suppressed":true}}"#,
        );
        let mut cfg = config();
        cfg.base_url = format!("http://{addr}");
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        assert!(channel
            .deliver(&event(EventType::ResourceUnhealthy, Severity::Critical))
            .is_ok());
    }

    #[test]
    fn invalid_credentials_are_rejected_without_retry() {
        let (addr, _captured) =
            responder(401, r#"{"code":401,"message":"invalid app credentials"}"#);
        let mut cfg = config();
        cfg.base_url = format!("http://{addr}");
        cfg.retries = 3;
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        let err = channel
            .deliver(&event(EventType::ResourceUnhealthy, Severity::Critical))
            .unwrap_err();
        // Terminal: Rejected means deliver_with_retry returns immediately.
        assert_eq!(err.kind, ChannelErrorKind::Rejected);
    }

    #[test]
    fn all_channel_failure_422_is_rejected() {
        // Herald writes code == HTTP status on error: "all channel deliveries
        // failed" arrives as HTTP 422, and we treat it as terminal (herald's
        // own at-least-once machinery owns the downstream retries).
        let (addr, _) = responder(422, r#"{"code":422,"message":"all channels failed"}"#);
        let mut cfg = config();
        cfg.base_url = format!("http://{addr}");
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        let err = channel
            .deliver(&event(EventType::ResourceUnhealthy, Severity::Critical))
            .unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Rejected);
    }

    #[test]
    fn server_errors_retry_then_fail() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = crate::test_http::read_full_request(&mut stream);
                let envelope = r#"{"code":500,"message":"boom"}"#;
                let response = format!(
                    "HTTP/1.1 500 X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{envelope}",
                    envelope.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        let mut cfg = config();
        cfg.base_url = format!("http://{addr}");
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        let err = channel
            .deliver(&event(EventType::ResourceUnhealthy, Severity::Critical))
            .unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Http);
    }

    #[test]
    fn construction_rejects_ssrf_blocked_target() {
        // Default policy blocks loopback; construction must fail fast
        // instead of POSTing dispatch payloads at internal addresses.
        let cfg = HeraldConfig {
            base_url: "http://127.0.0.1:8080".into(),
            app: "custos".into(),
            token: None,
            auth: HeraldAuth::ApiKey,
            category: "alerts".into(),
            audiences: vec!["log".into()],
            timeout_ms: 2000,
            retries: 0,
            backoff_ms: 1,
            ssrf: custos_config::SsrfPolicy::default_blocking(),
        };
        let err = match HeraldChannel::new("herald", cfg) {
            Err(e) => e,
            Ok(_) => panic!("expected construction to be rejected"),
        };
        assert_eq!(err.kind, ChannelErrorKind::Config);
        assert!(err.to_string().contains("SSRF"), "{err}");
    }

    #[test]
    fn allowlisted_herald_host_bypasses_ssrf() {
        // An allowlisted host name is accepted even though it resolves to
        // loopback; the channel then fails on connect (refused port) rather
        // than on the SSRF policy.
        let mut cfg = config();
        cfg.base_url = "http://localhost:9".into();
        cfg.ssrf.allowlist = vec!["localhost".into()];
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        let err = channel
            .deliver(&event(EventType::ResourceUnhealthy, Severity::Critical))
            .unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Transport);
    }

    #[test]
    fn two_hundred_without_envelope_is_rejected_not_retried() {
        // C4: a 2xx whose body is not herald's {"code":0} acceptance
        // envelope is a contract mismatch — the peer answered at the HTTP
        // level but did not confirm dispatch. Retrying cannot fix it, so it
        // is terminal and no identical re-POSTs happen.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(0usize));
        let cap = captured.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = crate::test_http::read_full_request(&mut stream);
                *cap.lock().unwrap() += 1;
                let body = "<html>gateway blurb</html>";
                let response = format!(
                    "HTTP/1.1 200 X\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        let mut cfg = config();
        cfg.base_url = format!("http://{addr}");
        cfg.retries = 3;
        let channel = HeraldChannel::new("herald", cfg).unwrap();
        let err = channel
            .deliver(&event(EventType::ResourceUnhealthy, Severity::Critical))
            .unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Rejected, "{err}");
        assert_eq!(
            *captured.lock().unwrap(),
            1,
            "no retry on contract mismatch"
        );
    }
}

//! Telegram notification channel — sends notifications via a bot's
//! `sendMessage` API using the mature `ureq` HTTP client.
//!
//! Configuration via `[notification.channels.<id>]`:
//! ```toml
//! type = "telegram"
//! bot_token = "123456:ABC-DEF..."
//! chat_id = "-1001234567890"
//! timeout_ms = 5000
//! # Optional outbound proxy for networks that block api.telegram.org:
//! # proxy = "http://user:pass@192.168.5.11:7890"
//! ```
//! One attempt per deliver: the queue engine owns retry and backoff
//! (§8.6, C19/C22); the parsed `retries`/`backoff_ms` fields are accepted
//! for config compatibility but no longer drive an in-channel ladder.
//! A Telegram bot token and a chat ID (or channel name) are required; the
//! bot must be a member of the target chat. Messages are truncated to the
//! Telegram 4096-character limit.

use crate::channel::{Channel, ChannelError, ChannelErrorKind};
use custos_model::NotificationEvent;
use serde_json::json;
use std::time::Duration;

const TG_API_BASE: &str = "https://api.telegram.org";
const MAX_MESSAGE_LEN: usize = 4096;

/// One Telegram delivery sink.
pub struct TelegramChannel {
    id: String,
    bot_token: String,
    chat_id: String,
    timeout_ms: u64,
    agent: ureq::Agent,
    api_base: String,
}

impl TelegramChannel {
    pub fn new(
        id: impl Into<String>,
        bot_token: impl Into<String>,
        chat_id: impl Into<String>,
        timeout_ms: u64,
    ) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(Duration::from_millis(timeout_ms)))
                .build(),
        );
        Self {
            id: id.into(),
            bot_token: bot_token.into(),
            chat_id: chat_id.into(),
            timeout_ms,
            agent,
            api_base: TG_API_BASE.to_owned(),
        }
    }

    /// Overrides the API base URL (for testing).
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Routes every delivery through an outbound HTTP(S) proxy of the form
    /// `http://[user:pass@]host:port`. Networks that block api.telegram.org
    /// directly (the live-acceptance deployment) deliver through the local
    /// proxy instead; credentials embedded in the URI are never logged (the
    /// scrubber collects the userinfo, and `Debug` redacts it).
    /// `ureq::Proxy` is the authoritative parser: a bad URI is a config
    /// error (terminal, never retried).
    pub fn with_proxy(mut self, proxy: &str) -> Result<Self, String> {
        let proxy = ureq::Proxy::new(proxy).map_err(|e| format!("invalid telegram proxy: {e}"))?;
        self.agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(Duration::from_millis(self.timeout_ms)))
                .proxy(Some(proxy))
                .build(),
        );
        Ok(self)
    }

    fn endpoint(&self) -> String {
        format!("{}/bot{}/sendMessage", self.api_base, self.bot_token)
    }

    /// Renders the notification as a Telegram message text. Respects the
    /// 4096-char limit.
    pub fn build_message(event: &NotificationEvent) -> String {
        let title = format!("[custos] {}: {}", event.event_type.as_str(), event.summary);
        if title.chars().count() > MAX_MESSAGE_LEN {
            format!(
                "{}…",
                title.chars().take(MAX_MESSAGE_LEN - 2).collect::<String>()
            )
        } else {
            title
        }
    }

    fn send_once(&self, payload: String) -> Result<(), ChannelError> {
        let request = self
            .agent
            .post(&self.endpoint())
            .header("Content-Type", "application/json");
        match request.send(payload.to_owned()) {
            Ok(response) => {
                let status = response.status().as_u16();
                // Telegram's Bot API returns HTTP 200 with `{"ok": true}`
                // only on acceptance. A 2xx IS the delivery confirmation:
                // a body that fails to read or parse must not turn a
                // delivered message into a dead letter (the old `.ok()`
                // swallow classified it Rejected). For non-2xx the body
                // carries the error detail.
                if (200..300).contains(&status) {
                    let _ = response.into_body().read_to_string();
                    return Ok(());
                }
                let code = response
                    .into_body()
                    .read_to_string()
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .and_then(|v| v.get("ok").and_then(|ok| ok.as_bool()));
                Err(ChannelError::new(
                    ChannelErrorKind::Rejected,
                    format!("telegram rejected: HTTP {status}, ok={code:?}"),
                ))
            }
            Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) => Err(
                ChannelError::new(ChannelErrorKind::Rejected, format!("telegram HTTP {code}")),
            ),
            Err(ureq::Error::StatusCode(code)) => Err(ChannelError::new(
                ChannelErrorKind::Http,
                format!("telegram HTTP {code}"),
            )),
            Err(other) => Err(ChannelError::new(
                ChannelErrorKind::Transport,
                format!("telegram transport error: {other}"),
            )),
        }
    }
}

impl Channel for TelegramChannel {
    fn id(&self) -> &str {
        &self.id
    }

    /// One attempt; the queue engine schedules any retry (§8.6, C19/C22).
    fn deliver(&self, event: &NotificationEvent) -> Result<(), ChannelError> {
        let message = Self::build_message(event);
        let payload = json!({
            "chat_id": self.chat_id,
            "text": message,
        })
        .to_string();
        self.send_once(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_model::{EventId, EventType, Severity};
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc, Mutex};

    fn event() -> NotificationEvent {
        NotificationEvent::new(
            EventId::new("evt-tg"),
            EventType::ResourceUnhealthy,
            Severity::Critical,
            "gateway unreachable",
            1_700_000_000_000,
        )
    }

    /// Single-shot Telegram API responder echoing a JSON body.
    fn responder(status: u16, body: &str) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        let body = body.to_owned();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = crate::test_http::read_full_request(&mut stream);
            cap.lock().unwrap().extend_from_slice(&request);
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (addr, captured)
    }

    fn channel_with_addr(addr: &str) -> TelegramChannel {
        TelegramChannel::new("tg", "123456:TEST-TOKEN", "-1001234567890", 2000)
            .with_api_base(format!("http://{addr}"))
    }

    #[test]
    fn build_message_includes_event_type_and_summary() {
        let msg = TelegramChannel::build_message(&event());
        assert!(msg.contains("resource.unhealthy"), "{msg}");
        assert!(msg.contains("gateway unreachable"), "{msg}");
    }

    #[test]
    fn message_truncated_at_limit() {
        let mut big = event();
        big.summary = "x".repeat(5000);
        let msg = TelegramChannel::build_message(&big);
        assert!(
            msg.chars().count() <= MAX_MESSAGE_LEN,
            "{}",
            msg.chars().count()
        );
    }

    #[test]
    fn delivers_with_bot_token_and_chat_id() {
        let (addr, captured) = responder(200, r#"{"ok":true}"#);
        let channel = channel_with_addr(&addr.to_string());
        channel.deliver(&event()).unwrap();

        let request = String::from_utf8_lossy(&captured.lock().unwrap().clone()).to_lowercase();
        assert!(
            request.contains("/bot123456:test-token/sendmessage"),
            "{request}"
        );
        assert!(request.contains("-1001234567890"), "{request}");
    }

    #[test]
    fn http_4xx_is_rejected_without_retry() {
        let (addr, captured) = responder(400, r#"{"ok":false,"error_code":400}"#);
        let cfg = channel_with_addr(&addr.to_string());
        let err = cfg.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Rejected);
        let _ = captured; // the responder served one request; single-shot means one attempt
    }

    #[test]
    fn invalid_proxy_scheme_is_a_config_error() {
        let channel = TelegramChannel::new("tg", "123456:TEST-TOKEN", "-1001234567890", 2000);
        let err = channel
            .with_proxy("ftp://127.0.0.1:7890")
            .err()
            .expect("non-http proxy must be refused");
        assert!(err.contains("invalid telegram proxy"), "{err}");
    }

    /// The api_base points at a local responder that would succeed, but the
    /// proxy is a refused port: only a delivery actually routed through the
    /// proxy fails, proving the proxy takes precedence over api_base.
    #[test]
    fn delivery_goes_through_the_configured_proxy() {
        let (addr, captured) = responder(200, r#"{"ok":true}"#);
        let channel = channel_with_addr(&addr.to_string())
            .with_proxy("http://127.0.0.1:9")
            .expect("http proxy parses");
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Transport, "{err:?}");
        // The responder never saw a request: the proxy intercepted it.
        assert!(captured.lock().unwrap().is_empty());
        let _ = addr;
    }

    #[test]
    fn server_5xx_is_http_retryable() {
        let (addr, captured) = responder(500, r#"{"ok":false,"error_code":500}"#);
        let channel = channel_with_addr(&addr.to_string());
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Http);
        // Retries=0 by default in test helper; responder only serves 1.
        assert!(!captured.lock().unwrap().is_empty());
    }

    /// LIVE delivery against the real api.telegram.org (2026-11-26,
    /// C-SET-UX-03/07 acceptance remainder): the isolated-instance run
    /// covered the config surface (pair validation, token masking) but a
    /// real delivery needs a real bot token, which only the user holds.
    /// Ignored by default; run with the environment set:
    ///   LIVE_TG_BOT_TOKEN=123456:ABC... LIVE_TG_CHAT_ID=-100... \
    ///   [LIVE_TG_PROXY=http://user:pass@host:port] \
    ///   cargo test -p custos-notify -- --ignored --nocapture
    /// LIVE_TG_PROXY is optional: networks that block api.telegram.org
    /// directly (DNS-poisoned, TCP 443 filtered) need the local outbound
    /// proxy, e.g. the router's authenticated mixed port.
    /// Sends exactly one message (single attempt; no retry ladder here).
    #[test]
    #[ignore]
    fn live_delivery_to_real_telegram() {
        let token = std::env::var("LIVE_TG_BOT_TOKEN").expect("LIVE_TG_BOT_TOKEN");
        let chat = std::env::var("LIVE_TG_CHAT_ID").expect("LIVE_TG_CHAT_ID");
        let mut channel = TelegramChannel::new("tg-acceptance", token, chat, 10_000);
        if let Ok(proxy) = std::env::var("LIVE_TG_PROXY") {
            channel = channel.with_proxy(&proxy).expect("LIVE_TG_PROXY");
        }

        let mut evt = event();
        evt.summary = format!(
            "C-SET-UX-03 live acceptance {}: sent by the ignored telegram test",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        channel.deliver(&evt).expect("live telegram delivery");
        println!("delivered: [custos] {}", evt.summary);
    }
}

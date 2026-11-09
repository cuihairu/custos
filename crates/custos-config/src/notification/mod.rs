//! Notification configuration (docs/guide/notification.md §8.2/§8.5) — an
//! independent top-level `[notification]` section, decoupled from recovery
//! rules. Recovery policy (per-resource) decides *how to fix*; notification
//! rules (global) decide *who hears about it* and *when*.
//!
//! The section is validated at load time (dashboard-cli-config.md §10.3
//! step 10 "验证通知渠道配置"): unknown fields are errors, rules may only
//! reference channels that exist, channel types are known, event/severity
//! names are checked, and secrets are never echoed by `Debug`.

use custos_model::{EventType, ResourceId, Severity};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

/// The validated `[notification]` section carried into the runtime.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotificationConfig {
    pub rules: Vec<NotificationRule>,
    /// Channel id → config. A rule dispatches to the channels it names.
    pub channels: BTreeMap<String, ChannelConfig>,
}

impl NotificationConfig {
    pub fn channel(&self, id: &str) -> Option<&ChannelConfig> {
        self.channels.get(id)
    }
}

/// One notification rule. Filters events and selects channels. Independent
/// from any recovery rule (docs/guide/notification.md §8.5).
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationRule {
    pub id: String,
    /// `None` matches every event type.
    pub event: Option<EventType>,
    /// Events at or above this severity match.
    pub severity_min: Severity,
    /// `None` matches every resource.
    pub resource: Option<ResourceId>,
    /// Channels to dispatch to (must exist in `NotificationConfig::channels`).
    pub channels: Vec<String>,
    /// Also write a line to the local structured audit log.
    pub log: bool,
    /// Suppress repeats of the same (event type, resource) fault within
    /// this window (§8.5 聚合并抑制重复通知). `0` disables dedup for this
    /// rule. Default 30 minutes.
    pub dedup_window_ms: u64,
}

/// A configured notification channel. Channels are typed; the transport for
/// each is a separate concern (docs/guide/notification.md §8.2).
#[derive(Clone, PartialEq)]
pub enum ChannelConfig {
    LocalLog { enabled: bool },
    Webhook(WebhookConfig),
    Smtp(SmtpConfig),
    Herald(HeraldConfig),
    Telegram(TelegramConfig),
}

impl std::fmt::Debug for ChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelConfig::LocalLog { enabled } => f
                .debug_struct("LocalLog")
                .field("enabled", enabled)
                .finish(),
            ChannelConfig::Webhook(cfg) => write!(f, "Webhook({cfg:?})"),
            ChannelConfig::Smtp(cfg) => write!(f, "Smtp({cfg:?})"),
            ChannelConfig::Herald(cfg) => write!(f, "Herald({cfg:?})"),
            ChannelConfig::Telegram(cfg) => write!(f, "Telegram({cfg:?})"),
        }
    }
}

/// Webhook channel settings (docs/guide/notification.md §8.2). Transport
/// (retry/backoff/HMAC/TLS/SSRF enforcement) is implemented by the channel
/// engine; this is the validated configuration it consumes.
#[derive(Clone, PartialEq)]
pub struct WebhookConfig {
    pub url: String,
    pub timeout_ms: u64,
    pub retries: u32,
    /// Base retry backoff; the channel doubles it per attempt.
    pub backoff_ms: u64,
    /// Extra headers (e.g. an Authorization header). Debug-redacted.
    pub headers: BTreeMap<String, String>,
    /// Optional shared secret for HMAC request signing. Debug-redacted.
    pub hmac_secret: Option<SecretString>,
    pub tls_verify: bool,
    pub ssrf: SsrfPolicy,
}

impl std::fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never echo auth headers or the HMAC secret (notification.md §8.7).
        f.debug_struct("WebhookConfig")
            .field("url", &self.url)
            .field("timeout_ms", &self.timeout_ms)
            .field("retries", &self.retries)
            .field("backoff_ms", &self.backoff_ms)
            .field("headers", &"[redacted]")
            .field("hmac_secret", &self.hmac_secret)
            .field("tls_verify", &self.tls_verify)
            .field("ssrf", &self.ssrf)
            .finish()
    }
}

/// Webhook SSRF protection policy (docs/guide/notification.md §8.2). The
/// transport resolves and enforces this; here we carry the policy.
#[derive(Debug, Clone, PartialEq)]
pub struct SsrfPolicy {
    pub block_private: bool,
    pub block_loopback: bool,
    pub block_redirects: bool,
    /// Hosts always allowed (bypass the block rules).
    pub allowlist: Vec<String>,
}

/// SMTP channel settings (docs/guide/notification.md §8.2).
#[derive(Clone, PartialEq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    /// Debug-redacted.
    pub password: Option<SecretString>,
    pub from: String,
    pub to: Vec<String>,
    pub tls: bool,
    pub connect_timeout_ms: u64,
    pub send_timeout_ms: u64,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password)
            .field("from", &self.from)
            .field("to", &self.to)
            .field("tls", &self.tls)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("send_timeout_ms", &self.send_timeout_ms)
            .finish()
    }
}

/// Which auth header carries the app token (github.com/cuihairu/herald
/// accepts both; the curl examples use `X-API-Key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeraldAuth {
    /// `X-API-Key: <token>` (default).
    #[default]
    ApiKey,
    /// `Authorization: Bearer <token>`.
    Bearer,
}

/// Herald channel settings (github.com/cuihairu/herald): POSTs each event to
/// the app dispatch endpoint `/api/v1/apps/{app}/dispatch`, mapping Custos
/// severity to herald urgency and fault events to herald's `down`/`ok`
/// state so herald-side dedup collapses repeats and a recovery flip resends.
#[derive(Clone, PartialEq)]
pub struct HeraldConfig {
    /// Herald base URL, e.g. `http://127.0.0.1:8080`.
    pub base_url: String,
    /// The registered app id.
    pub app: String,
    /// App token with `trigger` scope. Debug-redacted; omit when herald runs
    /// with auth disabled (loopback only per its config rules).
    pub token: Option<SecretString>,
    pub auth: HeraldAuth,
    /// Registered dispatch category (default `alerts`).
    pub category: String,
    /// Audiences to dispatch to (bare channel names, `user:<id>`,
    /// `group:<id>`; default `["log"]`).
    pub audiences: Vec<String>,
    pub timeout_ms: u64,
    pub retries: u32,
    /// Base retry backoff; the channel doubles it per attempt.
    pub backoff_ms: u64,
}

impl std::fmt::Debug for HeraldConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeraldConfig")
            .field("base_url", &self.base_url)
            .field("app", &self.app)
            .field("token", &self.token)
            .field("auth", &self.auth)
            .field("category", &self.category)
            .field("audiences", &self.audiences)
            .field("timeout_ms", &self.timeout_ms)
            .field("retries", &self.retries)
            .field("backoff_ms", &self.backoff_ms)
            .finish()
    }
}

/// Telegram channel settings: a bot token + chat id via the bot API.
#[derive(Clone, PartialEq)]
pub struct TelegramConfig {
    pub bot_token: SecretString,
    /// The target chat: a numeric ID, @channelname, or channel username.
    pub chat_id: String,
    pub timeout_ms: u64,
    pub retries: u32,
    pub backoff_ms: u64,
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never echo the bot token (notification.md 8.7).
        f.debug_struct("TelegramConfig")
            .field("bot_token", &self.bot_token)
            .field("chat_id", &self.chat_id)
            .field("timeout_ms", &self.timeout_ms)
            .field("retries", &self.retries)
            .field("backoff_ms", &self.backoff_ms)
            .finish()
    }
}

/// A config-provided secret. `Debug` and `Display` redact so load errors and
/// logs never echo credentials (docs/guide/notification.md §8.7); expose the
/// value only at the exact use site via [`SecretString::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The plaintext value. Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(SecretString(String::deserialize(deserializer)?))
    }
}

mod raw;

pub(crate) use raw::{build, RawNotification};

#[cfg(test)]
mod tests;

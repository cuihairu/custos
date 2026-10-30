//! Notification configuration (docs/guide/notification.md §8.2/§8.5) — an
//! independent top-level `[notification]` section, decoupled from recovery
//! rules. Recovery policy (per-resource) decides *how to fix*; notification
//! rules (global) decide *who hears about it* and *when*.
//!
//! The section is validated at load time (dashboard-cli-config.md §10.3
//! step 10 "验证通知渠道配置"): unknown fields are errors, rules may only
//! reference channels that exist, channel types are known, event/severity
//! names are checked, and secrets are never echoed by `Debug`.

use crate::ConfigError;
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
}

/// A configured notification channel. Channels are typed; the transport for
/// each is a separate concern (docs/guide/notification.md §8.2).
#[derive(Clone, PartialEq)]
pub enum ChannelConfig {
    LocalLog { enabled: bool },
    Webhook(WebhookConfig),
    Smtp(SmtpConfig),
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

// ---------------------------------------------------------------------------
// Raw (deserialized) shapes. Strictness comes from `deny_unknown_fields` on
// each per-type struct; the channel `type` discriminator is peeled off
// manually because internally-tagged enums cannot enforce unknown-field
// rejection (dashboard-cli-config.md §10.1).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNotification {
    #[serde(default)]
    rules: Vec<RawNotificationRule>,
    /// Channel id → table. Tables carry a `type` discriminator plus the
    /// per-type fields, validated on dispatch.
    #[serde(default)]
    channels: BTreeMap<String, toml::Table>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNotificationRule {
    id: String,
    /// Event wire name, or `*` / omitted for all events.
    #[serde(default)]
    event: Option<String>,
    /// `info` | `warning` | `critical` (default `info`).
    #[serde(default)]
    severity_min: Option<String>,
    /// Resource id, or `*` / omitted for all resources.
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    log: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLocalLog {
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWebhook {
    url: String,
    #[serde(default = "default_webhook_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_webhook_retries")]
    retries: u32,
    #[serde(default = "default_backoff_ms")]
    backoff_ms: u64,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    hmac_secret: Option<SecretString>,
    #[serde(default = "default_true")]
    tls_verify: bool,
    #[serde(default)]
    ssrf: Option<RawSsrfPolicy>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSsrfPolicy {
    #[serde(default = "default_true")]
    block_private: bool,
    #[serde(default = "default_true")]
    block_loopback: bool,
    #[serde(default = "default_true")]
    block_redirects: bool,
    #[serde(default)]
    allowlist: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSmtp {
    host: String,
    #[serde(default = "default_smtp_port")]
    port: u16,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<SecretString>,
    from: String,
    to: Vec<String>,
    #[serde(default = "default_true")]
    tls: bool,
    #[serde(default = "default_webhook_timeout_ms")]
    connect_timeout_ms: u64,
    #[serde(default = "default_webhook_timeout_ms")]
    send_timeout_ms: u64,
}

fn default_true() -> bool {
    true
}
fn default_webhook_timeout_ms() -> u64 {
    10_000
}
fn default_webhook_retries() -> u32 {
    3
}
fn default_backoff_ms() -> u64 {
    30_000
}
fn default_smtp_port() -> u16 {
    587
}

/// Builds the validated notification config. `known_resources` is the set of
/// configured resource IDs, used to reject rules that reference a resource
/// that does not exist (dangling references are load-time errors).
pub(crate) fn build(
    raw: Option<RawNotification>,
    known_resources: &std::collections::HashSet<&str>,
) -> Result<NotificationConfig, ConfigError> {
    let Some(raw) = raw else {
        return Ok(NotificationConfig::default());
    };

    // Channels first, so rules can be checked against the known ids.
    let mut channels = BTreeMap::new();
    for (id, table) in raw.channels {
        let path = format!("notification.channels.{id}");
        if id.trim().is_empty() {
            return Err(ConfigError::validation(
                "notification.channels",
                "channel id must not be empty",
            ));
        }
        let channel = build_channel(&id, table).map_err(|e| ConfigError::validation(&path, e))?;
        channels.insert(id, channel);
    }

    // Rules.
    let mut rules = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, raw_rule) in raw.rules.iter().enumerate() {
        let path = format!("notification.rules[{i}]");
        if raw_rule.id.trim().is_empty() {
            return Err(ConfigError::validation(
                format!("{path}.id"),
                "rule id must not be empty",
            ));
        }
        if !seen.insert(raw_rule.id.as_str()) {
            return Err(ConfigError::validation(
                format!("{path}.id"),
                format!("duplicate notification rule id `{}`", raw_rule.id),
            ));
        }
        let event = match raw_rule.event.as_deref() {
            None | Some("*") => None,
            Some(name) => Some(EventType::from_wire(name).ok_or_else(|| {
                ConfigError::validation(
                    format!("{path}.event"),
                    format!("unknown event type `{name}` (see docs/guide/notification.md §8.4)"),
                )
            })?),
        };
        let severity_min = match raw_rule.severity_min.as_deref() {
            None | Some("info") => Severity::Info,
            Some(name) => Severity::from_wire(name).ok_or_else(|| {
                ConfigError::validation(
                    format!("{path}.severity_min"),
                    format!("unknown severity `{name}` (expected info|warning|critical)"),
                )
            })?,
        };
        let resource = match raw_rule.resource.as_deref() {
            None | Some("*") => None,
            Some(id) => {
                if !known_resources.contains(id) {
                    return Err(ConfigError::validation(
                        format!("{path}.resource"),
                        format!("unknown resource `{id}`"),
                    ));
                }
                Some(ResourceId::new(id))
            }
        };
        for (c, channel) in raw_rule.channels.iter().enumerate() {
            if !channels.contains_key(channel) {
                return Err(ConfigError::validation(
                    format!("{path}.channels[{c}]"),
                    format!("rule references unknown channel `{channel}`"),
                ));
            }
        }
        rules.push(NotificationRule {
            id: raw_rule.id.clone(),
            event,
            severity_min,
            resource,
            channels: raw_rule.channels.clone(),
            log: raw_rule.log,
        });
    }

    Ok(NotificationConfig { rules, channels })
}

/// Peels the `type` discriminator and validates the per-type fields.
fn build_channel(id: &str, mut table: toml::Table) -> Result<ChannelConfig, String> {
    let kind = table
        .remove("type")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| format!("channel `{id}` must declare a `type` (local-log|webhook|smtp)"))?;
    let rest = toml::Value::Table(table);
    match kind.as_str() {
        "local-log" => {
            let raw: RawLocalLog = rest.try_into().map_err(|e| err_unknown_fields(&kind, &e))?;
            Ok(ChannelConfig::LocalLog {
                enabled: raw.enabled,
            })
        }
        "webhook" => {
            let raw: RawWebhook = rest.try_into().map_err(|e| err_unknown_fields(&kind, &e))?;
            if raw.url.is_empty() {
                return Err("webhook `url` must not be empty".to_owned());
            }
            let scheme = raw
                .url
                .split_once("://")
                .map(|(s, _)| s.to_ascii_lowercase());
            if !matches!(scheme.as_deref(), Some("http" | "https")) {
                return Err(format!(
                    "webhook `url` must be http or https, got `{}`",
                    raw.url
                ));
            }
            Ok(ChannelConfig::Webhook(WebhookConfig {
                url: raw.url,
                timeout_ms: raw.timeout_ms,
                retries: raw.retries,
                backoff_ms: raw.backoff_ms,
                headers: raw.headers,
                hmac_secret: raw.hmac_secret,
                tls_verify: raw.tls_verify,
                ssrf: raw
                    .ssrf
                    .map_or_else(SsrfPolicy::default_blocking, |p| SsrfPolicy {
                        block_private: p.block_private,
                        block_loopback: p.block_loopback,
                        block_redirects: p.block_redirects,
                        allowlist: p.allowlist,
                    }),
            }))
        }
        "smtp" => {
            let raw: RawSmtp = rest.try_into().map_err(|e| err_unknown_fields(&kind, &e))?;
            if raw.host.is_empty() {
                return Err("smtp `host` must not be empty".to_owned());
            }
            if raw.from.is_empty() {
                return Err("smtp `from` must not be empty".to_owned());
            }
            if raw.to.is_empty() {
                return Err("smtp `to` must list at least one recipient".to_owned());
            }
            Ok(ChannelConfig::Smtp(SmtpConfig {
                host: raw.host,
                port: raw.port,
                username: raw.username,
                password: raw.password,
                from: raw.from,
                to: raw.to,
                tls: raw.tls,
                connect_timeout_ms: raw.connect_timeout_ms,
                send_timeout_ms: raw.send_timeout_ms,
            }))
        }
        other => Err(format!(
            "channel `{id}` has unknown type `{other}` (expected local-log|webhook|smtp)"
        )),
    }
}

impl SsrfPolicy {
    fn default_blocking() -> Self {
        SsrfPolicy {
            block_private: true,
            block_loopback: true,
            block_redirects: true,
            allowlist: Vec::new(),
        }
    }
}

/// Formats a `toml::Value` conversion error, distinguishing an unknown-field
/// rejection from a type mismatch so the message names the actual cause.
fn err_unknown_fields(kind: &str, error: &toml::de::Error) -> String {
    let message = error.to_string();
    if message.contains("unknown field") {
        message
    } else {
        format!("invalid {kind} channel config: {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load;

    const BASE: &str = r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"
"#;

    #[test]
    fn no_notification_section_is_default_empty() {
        let config = load(BASE).unwrap();
        assert!(config.notification.rules.is_empty());
        assert!(config.notification.channels.is_empty());
    }

    #[test]
    fn full_section_parses_with_defaults() {
        let config = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "alert-exhaustion"
event = "recovery.exhausted"
severity_min = "warning"
resource = "web"
channels = ["ops-webhook", "audit"]
log = true

[[notification.rules]]
id = "all-info"
channels = []

[notification.channels.ops-webhook]
type = "webhook"
url = "https://ops.example.com/hook"
headers = { Authorization = "Bearer x" }
hmac_secret = "shh"

[notification.channels.audit]
type = "local-log"

[notification.channels.mail]
type = "smtp"
host = "smtp.example.com"
from = "custos@example.com"
to = ["ops@example.com"]
"#,
        )
        .unwrap();

        let notification = &config.notification;
        assert_eq!(notification.rules.len(), 2);
        let rule = &notification.rules[0];
        assert_eq!(rule.id, "alert-exhaustion");
        assert_eq!(rule.event, Some(EventType::RecoveryExhausted));
        assert_eq!(rule.severity_min, Severity::Warning);
        assert_eq!(rule.resource, Some(ResourceId::new("web")));
        assert_eq!(rule.channels, vec!["ops-webhook", "audit"]);
        assert!(rule.log);

        // Second rule: omitted event/severity/resource mean "everything,
        // info and up, any resource".
        let catch_all = &notification.rules[1];
        assert_eq!(catch_all.event, None);
        assert_eq!(catch_all.severity_min, Severity::Info);
        assert_eq!(catch_all.resource, None);

        // Channel defaults applied.
        match notification.channel("ops-webhook").unwrap() {
            ChannelConfig::Webhook(w) => {
                assert_eq!(w.url, "https://ops.example.com/hook");
                assert_eq!(w.timeout_ms, 10_000);
                assert_eq!(w.retries, 3);
                assert_eq!(w.backoff_ms, 30_000);
                assert!(w.tls_verify);
                assert!(w.ssrf.block_private);
                assert!(w.ssrf.block_loopback);
                assert!(w.ssrf.block_redirects);
            }
            other => panic!("expected webhook, got {other:?}"),
        }
        match notification.channel("audit").unwrap() {
            ChannelConfig::LocalLog { enabled } => assert!(*enabled),
            other => panic!("expected local-log, got {other:?}"),
        }
        match notification.channel("mail").unwrap() {
            ChannelConfig::Smtp(s) => {
                assert_eq!(s.port, 587);
                assert!(s.tls);
            }
            other => panic!("expected smtp, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_event_type() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
event = "resource.exploded"
channels = []
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown event type"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_severity() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
severity_min = "severe"
channels = []
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown severity"), "got: {err}");
    }

    #[test]
    fn rejects_dangling_channel_reference() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
channels = ["ghost"]
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown channel"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_resource_filter() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
resource = "ghost"
channels = []
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown resource"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_channel_type() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "carrier-pigeon"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown type"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_field_in_webhook_channel() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "webhook"
url = "https://example.com"
frobnicate = true
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field"), "got: {err}");
    }

    #[test]
    fn rejects_non_http_webhook_url() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "webhook"
url = "file:///etc/passwd"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("http or https"), "got: {err}");
    }

    #[test]
    fn rejects_smtp_without_recipients() {
        let err = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "smtp"
host = "smtp.example.com"
from = "a@example.com"
to = []
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("at least one"), "got: {err}");
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let config = load(
            r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "webhook"
url = "https://example.com"
headers = { Authorization = "Bearer super-secret" }
hmac_secret = "top-secret"
"#,
        )
        .unwrap();
        let debug = format!("{:?}", config.notification.channels.get("x").unwrap());
        assert!(!debug.contains("super-secret"), "leaked headers: {debug}");
        assert!(!debug.contains("top-secret"), "leaked secret: {debug}");
        assert!(debug.contains("[redacted]"), "no redaction: {debug}");
    }
}

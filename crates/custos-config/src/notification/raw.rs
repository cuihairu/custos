//! Raw notification-table unpacking and per-channel validation: parses the
//! `[notification]` TOML table, checks channel types and event/severity
//! names, and builds the validated channel configs.

use super::*;
use crate::ConfigError;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

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
    /// Suppress repeats of the same fault within this window; 0 disables.
    #[serde(default = "default_dedup_window_ms")]
    dedup_window_ms: u64,
}

/// Default same-fault suppression window (§8.5).
fn default_dedup_window_ms() -> u64 {
    30 * 60 * 1000
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHerald {
    base_url: String,
    app: String,
    #[serde(default)]
    token: Option<SecretString>,
    /// `api-key` (default) or `bearer`.
    #[serde(default)]
    auth: Option<String>,
    #[serde(default = "default_herald_category")]
    category: String,
    #[serde(default = "default_herald_audiences")]
    audiences: Vec<String>,
    #[serde(default = "default_webhook_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_webhook_retries")]
    retries: u32,
    #[serde(default = "default_backoff_ms")]
    backoff_ms: u64,
    #[serde(default)]
    ssrf: Option<RawSsrfPolicy>,
}

fn default_herald_category() -> String {
    "alerts".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTelegram {
    bot_token: SecretString,
    chat_id: String,
    #[serde(default = "default_webhook_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_webhook_retries")]
    retries: u32,
    #[serde(default = "default_backoff_ms")]
    backoff_ms: u64,
}

fn default_herald_audiences() -> Vec<String> {
    vec!["log".to_owned()]
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
    known_resources: &HashSet<&str>,
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
    let mut seen = HashSet::new();
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
            dedup_window_ms: raw_rule.dedup_window_ms,
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
        "herald" => {
            let raw: RawHerald = rest.try_into().map_err(|e| err_unknown_fields(&kind, &e))?;
            if raw.base_url.is_empty() {
                return Err("herald `base_url` must not be empty".to_owned());
            }
            let scheme = raw
                .base_url
                .split_once("://")
                .map(|(s, _)| s.to_ascii_lowercase());
            if !matches!(scheme.as_deref(), Some("http" | "https")) {
                return Err(format!(
                    "herald `base_url` must be http or https, got `{}`",
                    raw.base_url
                ));
            }
            if raw.app.trim().is_empty() {
                return Err("herald `app` must not be empty".to_owned());
            }
            if raw.audiences.is_empty() {
                return Err(
                    "herald `audiences` must list at least one audience (herald rejects empty)"
                        .to_owned(),
                );
            }
            if raw.category.trim().is_empty() {
                return Err("herald `category` must not be empty".to_owned());
            }
            let auth = match raw.auth.as_deref() {
                None => HeraldAuth::ApiKey,
                Some("api-key") => HeraldAuth::ApiKey,
                Some("bearer") => HeraldAuth::Bearer,
                Some(other) => {
                    return Err(format!(
                        "herald `auth` must be api-key or bearer, got `{other}`"
                    ))
                }
            };
            Ok(ChannelConfig::Herald(HeraldConfig {
                base_url: raw.base_url.trim_end_matches('/').to_owned(),
                app: raw.app,
                token: raw.token,
                auth,
                category: raw.category,
                audiences: raw.audiences,
                timeout_ms: raw.timeout_ms,
                retries: raw.retries,
                backoff_ms: raw.backoff_ms,
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
        "telegram" => {
            let raw: RawTelegram = rest.try_into().map_err(|e| err_unknown_fields(&kind, &e))?;
            if raw.chat_id.trim().is_empty() {
                return Err("telegram `chat_id` must not be empty".to_owned());
            }
            Ok(ChannelConfig::Telegram(TelegramConfig {
                bot_token: raw.bot_token,
                chat_id: raw.chat_id,
                timeout_ms: raw.timeout_ms,
                retries: raw.retries,
                backoff_ms: raw.backoff_ms,
            }))
        }
        other => Err(format!(
            "channel `{id}` has unknown type `{other}` (expected local-log|webhook|smtp|herald|telegram)"
        )),
    }
}

impl SsrfPolicy {
    /// The default policy: block private, loopback and redirect targets
    /// (docs/guide/notification.md §8.2 SSRF protection).
    pub fn default_blocking() -> Self {
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

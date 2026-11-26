//! Sensitive-value scrubbing (docs/guide/notification.md §8.7): tokens,
//! passwords, private keys and full auth headers must never reach logs or
//! the notification queue.
//!
//! The config types already redact their own `Debug` output; this module is
//! the second line of defense for strings that flow THROUGH the engine —
//! channel error messages are persisted as queue `last_error` and logged,
//! and a remote server (or a URL with embedded credentials) can echo a
//! secret back inside such a message. The engine collects every secret it
//! was configured with and replaces any occurrence with `[redacted]` before
//! the string is stored or logged.

use custos_config::{ChannelConfig, NotificationConfig};

/// The redaction marker.
pub const REDACTED: &str = "[redacted]";

/// Replaces occurrences of the collected secret values in free text.
#[derive(Debug, Clone, Default)]
pub struct Scrubber {
    needles: Vec<String>,
}

/// Extracts non-empty userinfo credentials from a URL so an error message
/// echoing the URL cannot leak them. Both the percent-encoded form (how the
/// URL serializes) and the decoded form (how a peer would echo the secret
/// back in an error string) are collected when they differ.
fn collect_url_credentials(url: &str, needles: &mut Vec<String>) {
    if let Ok(parsed) = url::Url::parse(url) {
        if !parsed.username().is_empty() {
            needles.push(parsed.username().to_owned());
            push_decoded(parsed.username(), needles);
        }
        if let Some(password) = parsed.password() {
            needles.push(password.to_owned());
            push_decoded(password, needles);
        }
    }
}

/// Decodes a percent-encoded credential and collects the plaintext form;
/// peers that received the decoded credential echo the decoded form, which
/// the encoded needle would never match. Invalid UTF-8 decodes are skipped.
fn push_decoded(encoded: &str, needles: &mut Vec<String>) {
    if let Ok(decoded) = percent_encoding::percent_decode_str(encoded).decode_utf8() {
        if decoded.as_ref() != encoded && !decoded.is_empty() {
            needles.push(decoded.into_owned());
        }
    }
}

impl Scrubber {
    /// Collects every secret configured on the notification channels: the
    /// webhook HMAC secret and all configured header values, the SMTP
    /// password, the herald app token, and any credentials embedded in
    /// channel URLs (userinfo in `https://user:pass@host`).
    pub fn from_config(config: &NotificationConfig) -> Self {
        let mut needles = Vec::new();
        for channel in config.channels.values() {
            match channel {
                ChannelConfig::Webhook(cfg) => {
                    if let Some(secret) = &cfg.hmac_secret {
                        needles.push(secret.expose().to_owned());
                    }
                    needles.extend(cfg.headers.values().cloned());
                    collect_url_credentials(&cfg.url, &mut needles);
                }
                ChannelConfig::Smtp(cfg) => {
                    if let Some(password) = &cfg.password {
                        needles.push(password.expose().to_owned());
                    }
                }
                ChannelConfig::Herald(cfg) => {
                    if let Some(token) = &cfg.token {
                        needles.push(token.expose().to_owned());
                    }
                    collect_url_credentials(&cfg.base_url, &mut needles);
                }
                ChannelConfig::Telegram(cfg) => {
                    needles.push(cfg.bot_token.expose().to_owned());
                    if let Some(proxy) = &cfg.proxy {
                        collect_url_credentials(proxy, &mut needles);
                    }
                }
                ChannelConfig::LocalLog { .. } => {}
            }
        }
        // Never treat an empty secret as a needle (it would redact
        // everything).
        needles.retain(|n| !n.is_empty());
        Self { needles }
    }

    /// True when nothing was collected (scrubbing is a no-op).
    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }

    /// Replaces every occurrence of every collected secret with
    /// `[redacted]`.
    pub fn scrub(&self, text: &str) -> String {
        let mut scrubbed = text.to_owned();
        for needle in &self.needles {
            if scrubbed.contains(needle.as_str()) {
                scrubbed = scrubbed.replace(needle.as_str(), REDACTED);
            }
        }
        scrubbed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_config::{
        HeraldConfig, SecretString, SmtpConfig, SsrfPolicy, TelegramConfig, WebhookConfig,
    };
    use std::collections::BTreeMap;

    fn config() -> NotificationConfig {
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), "Bearer super-token".to_owned());
        let mut channels = BTreeMap::new();
        channels.insert(
            "wh".to_owned(),
            ChannelConfig::Webhook(WebhookConfig {
                url: "https://example.com/hook".into(),
                timeout_ms: 1000,
                retries: 0,
                backoff_ms: 1,
                headers,
                hmac_secret: Some(SecretString::new("hmac-secret-value")),
                tls_verify: true,
                ssrf: SsrfPolicy::default_blocking(),
            }),
        );
        channels.insert(
            "mail".to_owned(),
            ChannelConfig::Smtp(SmtpConfig {
                host: "smtp.example.com".into(),
                port: 587,
                username: Some("user".into()),
                password: Some(SecretString::new("smtp-password")),
                from: "a@example.com".into(),
                to: vec!["b@example.com".into()],
                tls: true,
                connect_timeout_ms: 1000,
                send_timeout_ms: 1000,
            }),
        );
        channels.insert(
            "herald".to_owned(),
            ChannelConfig::Herald(HeraldConfig {
                base_url: "http://127.0.0.1:8080".into(),
                app: "custos".into(),
                token: Some(SecretString::new("herald-trigger-token")),
                auth: custos_config::HeraldAuth::ApiKey,
                category: "alerts".into(),
                audiences: vec!["log".into()],
                timeout_ms: 1000,
                retries: 0,
                backoff_ms: 1,
                ssrf: SsrfPolicy::default_blocking(),
            }),
        );
        channels.insert(
            "tg".to_owned(),
            ChannelConfig::Telegram(TelegramConfig {
                bot_token: SecretString::new("tg-bot-token"),
                chat_id: "@ops".into(),
                timeout_ms: 1000,
                retries: 0,
                backoff_ms: 1,
                proxy: Some("http://proxy-user:proxy-pass@127.0.0.1:7890".into()),
            }),
        );
        NotificationConfig {
            rules: vec![],
            channels,
        }
    }

    #[test]
    fn scrubs_every_collected_secret_kind() {
        let scrubber = Scrubber::from_config(&config());
        assert!(!scrubber.is_empty());

        let leaked = "webhook auth failed for Bearer super-token (hmac hmac-secret-value), smtp password smtp-password, herald token herald-trigger-token, telegram token tg-bot-token via proxy-user:proxy-pass";
        let scrubbed = scrubber.scrub(leaked);
        assert!(!scrubbed.contains("super-token"), "{scrubbed}");
        assert!(!scrubbed.contains("hmac-secret-value"), "{scrubbed}");
        assert!(!scrubbed.contains("smtp-password"), "{scrubbed}");
        assert!(!scrubbed.contains("herald-trigger-token"), "{scrubbed}");
        assert!(!scrubbed.contains("tg-bot-token"), "{scrubbed}");
        assert!(!scrubbed.contains("proxy-user"), "{scrubbed}");
        assert!(!scrubbed.contains("proxy-pass"), "{scrubbed}");
        assert_eq!(scrubbed.matches(REDACTED).count(), 7);
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let scrubber = Scrubber::from_config(&config());
        let clean = "herald rejected: HTTP 422, code Some(422)";
        assert_eq!(scrubber.scrub(clean), clean);
    }

    #[test]
    fn empty_scrubber_is_identity() {
        let scrubber = Scrubber::default();
        assert!(scrubber.is_empty());
        assert_eq!(scrubber.scrub("anything"), "anything");
    }

    #[test]
    fn url_userinfo_credentials_are_collected() {
        let mut channels = BTreeMap::new();
        channels.insert(
            "wh".to_owned(),
            ChannelConfig::Webhook(WebhookConfig {
                url: "https://deploy:leaky-pass@example.com/hook".into(),
                timeout_ms: 1000,
                retries: 0,
                backoff_ms: 1,
                headers: BTreeMap::new(),
                hmac_secret: None,
                tls_verify: true,
                ssrf: SsrfPolicy::default_blocking(),
            }),
        );
        let config = NotificationConfig {
            rules: vec![],
            channels,
        };
        let scrubber = Scrubber::from_config(&config);
        let scrubbed = scrubber.scrub("failed to reach https://deploy:leaky-pass@example.com/hook");
        assert!(!scrubbed.contains("leaky-pass"), "{scrubbed}");
        assert!(!scrubbed.contains("deploy:"), "{scrubbed}");
    }

    /// A peer that received the credential echoes the DECODED form ("p@ss"),
    /// not the encoded serialization ("p%40ss") the URL stores — both must
    /// be needles or the decoded echo leaks (e.g. a CONNECT proxy restating
    /// the credentials it was given in a failure message).
    #[test]
    fn percent_encoded_userinfo_is_collected_in_both_forms() {
        let mut channels = BTreeMap::new();
        channels.insert(
            "tg".to_owned(),
            ChannelConfig::Telegram(TelegramConfig {
                bot_token: SecretString::new("tg-bot-token"),
                chat_id: "@ops".into(),
                timeout_ms: 1000,
                retries: 0,
                backoff_ms: 1,
                proxy: Some("http://us%65r:p%40ss@127.0.0.1:7890".into()),
            }),
        );
        let config = NotificationConfig {
            rules: vec![],
            channels,
        };
        let scrubber = Scrubber::from_config(&config);
        // Encoded form (URL echo).
        let scrubbed = scrubber.scrub("connect failed for us%65r:p%40ss@127.0.0.1:7890");
        assert!(!scrubbed.contains("p%40ss"), "{scrubbed}");
        // Decoded form (peer echo).
        let scrubbed = scrubber.scrub("proxy rejected user with p@ss");
        assert!(!scrubbed.contains("p@ss"), "{scrubbed}");
        assert!(!scrubbed.contains("user"), "{scrubbed}");
    }

    #[test]
    fn empty_secrets_are_not_needles() {
        let mut channels = BTreeMap::new();
        channels.insert(
            "wh".to_owned(),
            ChannelConfig::Webhook(WebhookConfig {
                url: "https://example.com/hook".into(),
                timeout_ms: 1000,
                retries: 0,
                backoff_ms: 1,
                headers: BTreeMap::new(),
                hmac_secret: Some(SecretString::new("")),
                tls_verify: true,
                ssrf: SsrfPolicy::default_blocking(),
            }),
        );
        let config = NotificationConfig {
            rules: vec![],
            channels,
        };
        let scrubber = Scrubber::from_config(&config);
        assert!(scrubber.is_empty(), "empty secret must not become a needle");
    }
}

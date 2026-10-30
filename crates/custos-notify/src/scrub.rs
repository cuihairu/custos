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
/// echoing the URL cannot leak them.
fn collect_url_credentials(url: &str, needles: &mut Vec<String>) {
    if let Ok(parsed) = url::Url::parse(url) {
        if !parsed.username().is_empty() {
            needles.push(parsed.username().to_owned());
        }
        if let Some(password) = parsed.password() {
            needles.push(password.to_owned());
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
    use custos_config::{HeraldConfig, SecretString, SmtpConfig, SsrfPolicy, WebhookConfig};
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

        let leaked = "webhook auth failed for Bearer super-token (hmac hmac-secret-value), smtp password smtp-password, herald token herald-trigger-token";
        let scrubbed = scrubber.scrub(leaked);
        assert!(!scrubbed.contains("super-token"), "{scrubbed}");
        assert!(!scrubbed.contains("hmac-secret-value"), "{scrubbed}");
        assert!(!scrubbed.contains("smtp-password"), "{scrubbed}");
        assert!(!scrubbed.contains("herald-trigger-token"), "{scrubbed}");
        assert_eq!(scrubbed.matches(REDACTED).count(), 4);
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

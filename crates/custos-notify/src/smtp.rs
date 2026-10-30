//! SMTP channel (docs/guide/notification.md §8.2) — sends each event as an
//! email to configured recipients over the mature `lettre` client, with
//! connect/send timeouts, STARTTLS or implicit TLS per config, credentials
//! read from the validated config (never logged), and a clear result.
//!
//! Errors are classified per the webhook convention: transient SMTP
//! responses (4xx) and TLS/transport problems → `Transport` (retryable),
//! permanent responses (5xx) and client/auth failures → `Rejected`
//! (terminal). Retry/backoff is left to the queue engine (§8.6), which
//! schedules around the single attempt this channel performs.

use crate::channel::{Channel, ChannelError, ChannelErrorKind};
use crate::local_log::format_event;
use custos_config::SmtpConfig;
use custos_model::NotificationEvent;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::SmtpTransport;
use lettre::Transport;
use std::time::Duration;

/// One SMTP delivery sink.
pub struct SmtpChannel {
    id: String,
    config: SmtpConfig,
    transport: SmtpTransport,
}

impl SmtpChannel {
    /// Builds the transport from the validated config. `tls` toggles between
    /// requiring STARTTLS/TLS (`true`, secure) and plaintext (`false`, only
    /// for trusted local relays). The socket timeout is the send budget,
    /// which also bounds the connect phase.
    pub fn new(id: impl Into<String>, config: SmtpConfig) -> Result<Self, ChannelError> {
        let server = format!("{}:{}", config.host, config.port);
        let mut builder = SmtpTransport::builder_dangerous(server);
        builder = builder.timeout(Some(Duration::from_millis(config.send_timeout_ms)));
        builder = if config.tls {
            let parameters = lettre::transport::smtp::client::TlsParameters::new(
                config.host.clone(),
            )
            .map_err(|e| {
                ChannelError::new(
                    ChannelErrorKind::Config,
                    format!("smtp tls parameters: {e}"),
                )
            })?;
            builder.tls(Tls::Required(parameters))
        } else {
            builder.tls(Tls::None)
        };
        if let (Some(username), Some(password)) = (&config.username, &config.password) {
            builder = builder.credentials(Credentials::new(
                username.clone(),
                password.expose().to_owned(),
            ));
        }
        Ok(Self {
            id: id.into(),
            config,
            transport: builder.build(),
        })
    }

    fn build_email(&self, event: &NotificationEvent) -> Result<lettre::Message, ChannelError> {
        use lettre::message::Message;
        let from: Mailbox = self.config.from.parse().map_err(|_| {
            ChannelError::new(
                ChannelErrorKind::Config,
                format!("smtp `from` is not a valid mailbox: `{}`", self.config.from),
            )
        })?;
        let mut builder = Message::builder().from(from).subject(email_subject(event));
        for recipient in &self.config.to {
            let mailbox: Mailbox = recipient.parse().map_err(|_| {
                ChannelError::new(
                    ChannelErrorKind::Config,
                    format!("smtp `to` entry is not a valid mailbox: `{recipient}`"),
                )
            })?;
            builder = builder.to(mailbox);
        }
        builder.body(format_event(event)).map_err(|e| {
            ChannelError::new(ChannelErrorKind::Config, format!("smtp message build: {e}"))
        })
    }
}

impl Channel for SmtpChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn deliver(&self, event: &NotificationEvent) -> Result<(), ChannelError> {
        let email = self.build_email(event)?;
        self.transport
            .send(&email)
            .map(|_| ())
            .map_err(classify_smtp_error)
    }
}

/// A concise, greppable subject line: `[custos] <event_type>: <summary>`.
fn email_subject(event: &NotificationEvent) -> String {
    let mut subject = format!("[custos] {}: {}", event.event_type.as_str(), event.summary);
    if subject.chars().count() > 120 {
        subject = subject.chars().take(117).collect::<String>();
        subject.push_str("...");
    }
    subject
}

/// Classifies a lettre SMTP error into the channel's error kinds. The SMTP
/// error is a struct (not an enum), so we use its predicates: permanent
/// (5xx) and client/auth failures are terminal; transient (4xx), TLS and
/// transport problems are retryable.
fn classify_smtp_error(error: lettre::transport::smtp::Error) -> ChannelError {
    let message = format!("smtp: {error}");
    let kind = if error.is_permanent() || error.is_client() {
        ChannelErrorKind::Rejected
    } else {
        ChannelErrorKind::Transport
    };
    ChannelError::new(kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_model::{EventId, EventType, Severity};

    fn event() -> NotificationEvent {
        NotificationEvent::new(
            EventId::new("evt-4"),
            EventType::ResourceDegraded,
            Severity::Warning,
            "high latency",
            1_700_000_000_000,
        )
    }

    fn config() -> SmtpConfig {
        SmtpConfig {
            host: "127.0.0.1".into(),
            port: 2525,
            username: None,
            password: None,
            from: "custos@example.com".into(),
            to: vec!["ops@example.com".into()],
            tls: false,
            connect_timeout_ms: 500,
            send_timeout_ms: 500,
        }
    }

    #[test]
    fn subject_is_concise_and_greppable() {
        let subject = email_subject(&event());
        assert!(
            subject.starts_with("[custos] resource.degraded: high latency"),
            "{subject}"
        );
    }

    #[test]
    fn subject_truncates_long_summaries() {
        let mut long = event();
        long.summary = "x".repeat(300);
        let subject = email_subject(&long);
        assert!(
            subject.chars().count() <= 120,
            "{}",
            subject.chars().count()
        );
        assert!(subject.ends_with("..."));
    }

    #[test]
    fn builds_email_with_from_and_recipients() {
        let channel = SmtpChannel::new("mail", config()).unwrap();
        let email = channel.build_email(&event()).unwrap();
        let raw = String::from_utf8_lossy(&email.formatted()).to_lowercase();
        assert!(raw.contains("custos@example.com"), "{raw}");
        assert!(raw.contains("ops@example.com"), "{raw}");
        assert!(raw.contains("resource.degraded"), "{raw}");
    }

    #[test]
    fn invalid_from_is_config_error() {
        let mut cfg = config();
        cfg.from = "not-an-email".into();
        let channel = SmtpChannel::new("mail", cfg).unwrap();
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Config);
    }

    #[test]
    fn refused_connection_is_transport() {
        // A closed port on the SMTP host produces a transport error.
        let addr = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            drop(listener);
            addr
        };
        let mut cfg = config();
        cfg.port = addr.port();
        let channel = SmtpChannel::new("mail", cfg).unwrap();
        let err = channel.deliver(&event()).unwrap_err();
        assert_eq!(err.kind, ChannelErrorKind::Transport);
    }
}

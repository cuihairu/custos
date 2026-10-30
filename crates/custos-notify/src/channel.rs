//! The channel dispatch interface (docs/guide/notification.md §8.1-8.2).
//!
//! A channel is one delivery sink (local log, webhook, SMTP, console, or a
//! third-party plugin). Delivery failures must never block recovery
//! (§8.1): every failure is a returned error carrying a kind, so the queue
//! engine (retry/backoff/dead-letter, §8.6) can decide what to do without
//! panics or unwinding.

use custos_model::NotificationEvent;

/// Why a delivery failed. The retry engine keys on this: `Transport` and
/// `Http` are retryable per backoff; `Rejected` and `Config` are terminal
/// (retrying cannot fix them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelErrorKind {
    /// Could not reach the remote end (connect refused, DNS, timeout).
    /// Retryable.
    Transport,
    /// The remote answered with an HTTP failure. Retryable unless the
    /// status says otherwise (the channel decides).
    Http,
    /// The remote definitively rejected the payload (4xx, auth failure).
    /// Terminal.
    Rejected,
    /// The channel is misconfigured (bad URL, missing recipient). Terminal.
    Config,
}

/// One delivery failure. Carries the kind plus a human-readable message
/// that must not contain secrets (docs/guide/notification.md §8.7).
#[derive(Debug, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct ChannelError {
    pub kind: ChannelErrorKind,
    pub message: String,
}

impl ChannelError {
    pub fn new(kind: ChannelErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// One delivery sink. Implementations are synchronous and bounded by their
/// own timeouts; the queue engine schedules and retries around them.
pub trait Channel: Send + Sync {
    /// The channel id from `[notification.channels.<id>]`.
    fn id(&self) -> &str;

    /// Delivers one event. Must not panic; must not block longer than its
    /// configured timeout.
    fn deliver(&self, event: &NotificationEvent) -> Result<(), ChannelError>;
}

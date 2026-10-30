//! Persistence port for the notification queue (docs/guide/notification.md
//! §8.6).
//!
//! `custos-notify` depends only on this trait, never on a concrete store:
//! that keeps the dependency graph acyclic when `custos-engine` (which also
//! depends on `custos-store`) emits events into the notify engine.
//! `custos-store` implements the trait over its SQLite `notification_queue`.

use custos_model::{NotificationEvent, NotificationState};

/// A queue entry claimed for delivery. `state` is always `Sending` after a
/// claim; `attempts` drives the retry backoff.
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedNotification {
    pub id: u64,
    pub channel: String,
    pub event: NotificationEvent,
    pub state: NotificationState,
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// The persistence operations the notification engine needs. Implementations
/// must be safe to call from the engine's single delivery worker.
pub trait NotificationQueue {
    fn enqueue(
        &self,
        channel: &str,
        event: &NotificationEvent,
        now_unix_ms: u64,
    ) -> Result<u64, QueueError>;

    /// Claims due entries (see `custos-store`) and moves them to `sending`.
    fn claim_due(
        &self,
        now_unix_ms: u64,
        limit: u32,
    ) -> Result<Vec<QueuedNotification>, QueueError>;

    fn mark_delivered(&self, id: u64, at_unix_ms: u64) -> Result<(), QueueError>;

    fn mark_retry(
        &self,
        id: u64,
        next_attempt_at_unix_ms: u64,
        error: &str,
    ) -> Result<(), QueueError>;

    fn mark_dead(&self, id: u64, error: &str, now_unix_ms: u64) -> Result<(), QueueError>;
}

// Note: trait methods take `&self` (not `&mut self`) because `custos-store`
// implements them over an interior-mutable connection pool, and the engine
// calls them through a shared reference.

/// A persistence failure. The engine logs it and moves on — it never blocks
/// recovery (docs/guide/notification.md §8.1).
#[derive(Debug, thiserror::Error)]
#[error("notification queue: {0}")]
pub struct QueueError(pub String);

impl QueueError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl<T: NotificationQueue + ?Sized> NotificationQueue for std::sync::Arc<T> {
    fn enqueue(
        &self,
        channel: &str,
        event: &custos_model::NotificationEvent,
        now_unix_ms: u64,
    ) -> Result<u64, QueueError> {
        (**self).enqueue(channel, event, now_unix_ms)
    }

    fn claim_due(
        &self,
        now_unix_ms: u64,
        limit: u32,
    ) -> Result<Vec<QueuedNotification>, QueueError> {
        (**self).claim_due(now_unix_ms, limit)
    }

    fn mark_delivered(&self, id: u64, at_unix_ms: u64) -> Result<(), QueueError> {
        (**self).mark_delivered(id, at_unix_ms)
    }

    fn mark_retry(
        &self,
        id: u64,
        next_attempt_at_unix_ms: u64,
        error: &str,
    ) -> Result<(), QueueError> {
        (**self).mark_retry(id, next_attempt_at_unix_ms, error)
    }

    fn mark_dead(&self, id: u64, error: &str, now_unix_ms: u64) -> Result<(), QueueError> {
        (**self).mark_dead(id, error, now_unix_ms)
    }
}

//! Store concern: notification queue persistence, the NotificationQueue trait bridge and row helpers.

use super::*;

impl Store {
    /// Enqueues one event for one channel. The event JSON is stored as the
    /// payload; the event id is kept as a column so dedup (docs/guide/
    /// notification.md §8.6) can query it without parsing.
    pub fn enqueue_notification(
        &self,
        channel: &str,
        event: &NotificationEvent,
        now_unix_ms: u64,
    ) -> Result<u64, StoreError> {
        let payload = serde_json::to_string(event)
            .map_err(|e| StoreError::Corrupt("notification_queue payload".into(), e))?;
        self.block(move |pool| async move {
            let result = query(
                "INSERT INTO notification_queue \
                 (resource_id, channel, payload, event_id, state, attempts, \
                  next_attempt_at_unix_ms, last_error, created_at_unix_ms, \
                  delivered_at_unix_ms, updated_at_unix_ms) \
                 VALUES (?, ?, ?, ?, 'pending', 0, NULL, NULL, ?, NULL, ?)",
            )
            .bind(event.resource_id.as_ref().map(|r| r.as_str()))
            .bind(channel)
            .bind(payload)
            .bind(event.event_id.as_str())
            .bind(now_unix_ms as i64)
            .bind(now_unix_ms as i64)
            .execute(&pool)
            .await?;
            Ok(result.last_insert_rowid() as u64)
        })
        .map_err(Into::into)
    }
    /// Claims due queue entries for delivery: `pending`/`retry_scheduled`
    /// entries whose `next_attempt_at` is due, plus `sending` entries whose
    /// delivery lease has lapsed (crash mid-delivery). Claimed entries move
    /// to `sending` with a fresh lease of `lease_ms` — sized by the caller
    /// to outlive the channel's own retry ladder, so a live delivery is
    /// never reclaimed and delivered concurrently (C7). Pre-lease rows
    /// (NULL lease) fall back to the old staleness window.
    pub fn claim_due_notifications(
        &self,
        now_unix_ms: u64,
        limit: u32,
        lease_ms: u64,
    ) -> Result<Vec<NotificationQueueRow>, StoreError> {
        self.block(move |pool| async move {
            let mut tx = pool.begin().await?;
            let stale_before = (now_unix_ms as i64) - STALE_SENDING_MS;
            let lease_until = (now_unix_ms as i64).saturating_add(lease_ms as i64);
            let ids: Vec<i64> = query(
                "SELECT id FROM notification_queue \
                 WHERE (state IN ('pending', 'retry_scheduled') \
                        AND (next_attempt_at_unix_ms IS NULL OR next_attempt_at_unix_ms <= ?)) \
                    OR (state = 'sending' AND ( \
                        (lease_until_unix_ms IS NOT NULL AND lease_until_unix_ms < ?) \
                        OR (lease_until_unix_ms IS NULL AND updated_at_unix_ms < ?))) \
                 ORDER BY id LIMIT ?",
            )
            .bind(now_unix_ms as i64)
            .bind(now_unix_ms as i64)
            .bind(stale_before)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|row| row.get::<i64, _>(0))
            .collect();
            for id in &ids {
                query(
                    "UPDATE notification_queue SET state = 'sending', \
                     updated_at_unix_ms = ?, lease_until_unix_ms = ? WHERE id = ?",
                )
                .bind(now_unix_ms as i64)
                .bind(lease_until)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            let mut rows = Vec::new();
            for id in ids {
                if let Some(row) = fetch_notification_row(&pool, id).await? {
                    rows.push(row);
                }
            }
            Ok(rows)
        })
        .map_err(Into::into)
    }
    /// Marks an entry delivered. Only a claimed (`sending`) entry may
    /// transition: a late call from a superseded claim no-ops instead of
    /// overwriting an already-delivered row (or flipping it back).
    pub fn mark_notification_delivered(&self, id: u64, at_unix_ms: u64) -> Result<(), StoreError> {
        self.block(move |pool| async move {
            query(
                "UPDATE notification_queue SET state = 'delivered', \
                 delivered_at_unix_ms = ?, updated_at_unix_ms = ? \
                 WHERE id = ? AND state = 'sending'",
            )
            .bind(at_unix_ms as i64)
            .bind(at_unix_ms as i64)
            .bind(id as i64)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .map_err(Into::into)
    }
    /// Increments the attempt count and schedules a retry. Same `sending`
    /// lease guard as [`mark_notification_delivered`]: a delivered entry is
    /// never flipped back to a retry by a superseded claim.
    pub fn mark_notification_retry(
        &self,
        id: u64,
        next_attempt_at_unix_ms: u64,
        error: &str,
        now_unix_ms: u64,
    ) -> Result<(), StoreError> {
        self.block(move |pool| async move {
            query(
                "UPDATE notification_queue SET state = 'retry_scheduled', \
                 attempts = attempts + 1, next_attempt_at_unix_ms = ?, \
                 last_error = ?, updated_at_unix_ms = ? \
                 WHERE id = ? AND state = 'sending'",
            )
            .bind(next_attempt_at_unix_ms as i64)
            .bind(error)
            .bind(now_unix_ms as i64)
            .bind(id as i64)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .map_err(Into::into)
    }
    /// Marks an entry dead-lettered (retries exhausted). A manual retry can
    /// resurrect it via [`Store::retry_notification`].
    pub fn mark_notification_dead(
        &self,
        id: u64,
        error: &str,
        now_unix_ms: u64,
    ) -> Result<(), StoreError> {
        self.block(move |pool| async move {
            query(
                "UPDATE notification_queue SET state = 'dead_letter', \
                 attempts = attempts + 1, last_error = ?, updated_at_unix_ms = ? \
                 WHERE id = ?",
            )
            .bind(error)
            .bind(now_unix_ms as i64)
            .bind(id as i64)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .map_err(Into::into)
    }
    /// Re-schedules a dead-lettered entry as pending for an immediate
    /// attempt (the CLI manual-retry path, docs/guide/notification.md §8.6).
    pub fn retry_notification(&self, id: u64, now_unix_ms: u64) -> Result<(), StoreError> {
        self.block(move |pool| async move {
            query(
                "UPDATE notification_queue SET state = 'pending', \
                 next_attempt_at_unix_ms = NULL, last_error = NULL, \
                 updated_at_unix_ms = ? WHERE id = ?",
            )
            .bind(now_unix_ms as i64)
            .bind(id as i64)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .map_err(Into::into)
    }
    /// Recent queue entries, newest first, for the CLI status view.
    pub fn list_notifications(&self, limit: u32) -> Result<Vec<NotificationQueueRow>, StoreError> {
        self.block(move |pool| async move {
            let ids: Vec<i64> = query("SELECT id FROM notification_queue ORDER BY id DESC LIMIT ?")
                .bind(limit)
                .fetch_all(&pool)
                .await?
                .into_iter()
                .map(|row| row.get::<i64, _>(0))
                .collect();
            let mut rows = Vec::new();
            for id in ids {
                if let Some(row) = fetch_notification_row(&pool, id).await? {
                    rows.push(row);
                }
            }
            Ok(rows)
        })
        .map_err(Into::into)
    }
    /// Per-state counts for `custos notify status`.
    pub fn notification_counts(&self) -> Result<NotificationCounts, StoreError> {
        self.block(|pool| async move {
            let mut counts = NotificationCounts::default();
            let rows: Vec<(String, i64)> =
                query("SELECT state, COUNT(*) AS n FROM notification_queue GROUP BY state")
                    .fetch_all(&pool)
                    .await?
                    .into_iter()
                    .map(|row| (row.get::<String, _>(0), row.get::<i64, _>(1)))
                    .collect();
            for (state, n) in rows {
                match NotificationState::from_wire(&state) {
                    Some(NotificationState::Pending) => counts.pending = n as u64,
                    Some(NotificationState::Sending) => counts.sending = n as u64,
                    Some(NotificationState::Delivered) => counts.delivered = n as u64,
                    Some(NotificationState::RetryScheduled) => counts.retry_scheduled = n as u64,
                    Some(NotificationState::DeadLetter) => counts.dead_letter = n as u64,
                    None => {}
                }
            }
            Ok(counts)
        })
        .map_err(Into::into)
    }
}

impl custos_notify::NotificationQueue for Store {
    fn enqueue(
        &self,
        channel: &str,
        event: &NotificationEvent,
        now_unix_ms: u64,
    ) -> Result<u64, custos_notify::QueueError> {
        self.enqueue_notification(channel, event, now_unix_ms)
            .map_err(|e| custos_notify::QueueError::new(e.to_string()))
    }

    fn claim_due(
        &self,
        now_unix_ms: u64,
        limit: u32,
        lease_ms: u64,
    ) -> Result<Vec<custos_notify::QueuedNotification>, custos_notify::QueueError> {
        self.claim_due_notifications(now_unix_ms, limit, lease_ms)
            .map(|rows| {
                rows.into_iter()
                    .map(|r| custos_notify::QueuedNotification {
                        id: r.id,
                        channel: r.channel,
                        event: r.event,
                        state: r.state,
                        attempts: r.attempts,
                        last_error: r.last_error,
                    })
                    .collect()
            })
            .map_err(|e| custos_notify::QueueError::new(e.to_string()))
    }

    fn mark_delivered(&self, id: u64, at_unix_ms: u64) -> Result<(), custos_notify::QueueError> {
        self.mark_notification_delivered(id, at_unix_ms)
            .map_err(|e| custos_notify::QueueError::new(e.to_string()))
    }

    fn mark_retry(
        &self,
        id: u64,
        next_attempt_at_unix_ms: u64,
        error: &str,
        now_unix_ms: u64,
    ) -> Result<(), custos_notify::QueueError> {
        self.mark_notification_retry(id, next_attempt_at_unix_ms, error, now_unix_ms)
            .map_err(|e| custos_notify::QueueError::new(e.to_string()))
    }

    fn mark_dead(
        &self,
        id: u64,
        error: &str,
        now_unix_ms: u64,
    ) -> Result<(), custos_notify::QueueError> {
        self.mark_notification_dead(id, error, now_unix_ms)
            .map_err(|e| custos_notify::QueueError::new(e.to_string()))
    }
}

const STALE_SENDING_MS: i64 = 60_000;

type NotificationRawRow = (
    String,
    String,
    Option<String>,
    String,
    i64,
    Option<i64>,
    Option<String>,
    i64,
    Option<i64>,
);

async fn fetch_notification_row(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<NotificationQueueRow>, sqlx::Error> {
    let row: Option<NotificationRawRow> = query_as(
        "SELECT channel, payload, event_id, state, attempts, \
             next_attempt_at_unix_ms, last_error, created_at_unix_ms, \
             delivered_at_unix_ms FROM notification_queue WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((
        channel,
        payload,
        _event_id,
        state,
        attempts,
        next_at,
        last_error,
        created_at,
        delivered_at,
    )) = row
    else {
        return Ok(None);
    };
    let event = serde_json::from_str(&payload).map_err(|e| {
        sqlx::Error::Decode(Box::new(StoreError::Corrupt(
            format!("notification_queue row {id} payload"),
            e,
        )))
    })?;
    Ok(Some(NotificationQueueRow {
        id: id as u64,
        channel,
        event,
        state: NotificationState::from_wire(&state).unwrap_or(NotificationState::Pending),
        attempts: attempts as u32,
        next_attempt_at_unix_ms: next_at.map(|v| v as u64),
        last_error,
        created_at_unix_ms: created_at as u64,
        delivered_at_unix_ms: delivered_at.map(|v| v as u64),
    }))
}

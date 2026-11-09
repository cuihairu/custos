//! Spooled dispatch (C18): the event emitter runs on the engine's tick
//! thread, and a dispatch does synchronous SQLite inserts — a burst of
//! events serializes fsyncs on the one tick. The spool hands events to a
//! dedicated worker thread, so the emitter pays only a channel handoff; a
//! saturated spool falls back to one inline dispatch (backpressure) rather
//! than dropping the event (§8.1: notifications never block recovery, and
//! none are lost).

use crate::engine::NotificationEngine;
use crate::queue::NotificationQueue;
use custos_model::NotificationEvent;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;

/// Hands events to a worker thread that owns the engine dispatch. Dropping
/// the spool closes the channel; the worker drains what is queued and exits.
pub struct SpooledDispatch<Q: NotificationQueue + Send + Sync + 'static> {
    tx: SyncSender<NotificationEvent>,
    /// Shared with the worker; used by the inline fallback path.
    engine: Arc<NotificationEngine<Q>>,
}

impl<Q: NotificationQueue + Send + Sync + 'static> SpooledDispatch<Q> {
    /// Spawns the dispatch worker. `capacity` bounds the pending-event
    /// spool; bursts beyond it degrade the caller to inline dispatches.
    pub fn spawn(engine: Arc<NotificationEngine<Q>>, capacity: usize) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(capacity);
        let worker_engine = engine.clone();
        std::thread::Builder::new()
            .name("notify-dispatch".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    let report = worker_engine.dispatch(&event, event.occurred_at_unix_ms);
                    if report.enqueued > 0 || report.suppressed > 0 {
                        tracing::debug!(
                            target: "custos::runtime",
                            event = event.event_type.as_str(),
                            enqueued = report.enqueued,
                            suppressed = report.suppressed,
                            "notification dispatched"
                        );
                    }
                }
            })
            .expect("spawn notify dispatch worker");
        Self { tx, engine }
    }

    /// Hands the event to the worker. Never blocks on SQLite: a spool with
    /// room takes the handoff; a saturated spool falls back to one inline
    /// dispatch instead of dropping the event. The inline event may jump
    /// queued ones in the dedup window under extreme overload — the
    /// alternative is losing the notification.
    pub fn dispatch(&self, event: &NotificationEvent) {
        match self.tx.try_send(event.clone()) {
            Ok(()) => {}
            Err(TrySendError::Full(event)) => self.inline(&event),
            Err(TrySendError::Disconnected(event)) => self.inline(&event),
        }
    }

    /// One synchronous dispatch on the caller thread (the pre-C18 behavior),
    /// used only when the worker cannot take the event.
    fn inline(&self, event: &NotificationEvent) {
        let report = self.engine.dispatch(event, event.occurred_at_unix_ms);
        if report.enqueued > 0 || report.suppressed > 0 {
            tracing::debug!(
                target: "custos::runtime",
                event = event.event_type.as_str(),
                enqueued = report.enqueued,
                suppressed = report.suppressed,
                "notification dispatched inline (spool saturated)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{NotificationQueue, QueueError, QueuedNotification};
    use custos_config::{ChannelConfig, NotificationConfig};
    use custos_model::{EventId, EventType, ResourceId, Severity};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Queue that counts enqueues without any I/O.
    struct CountingQueue {
        count: Mutex<usize>,
    }

    impl NotificationQueue for CountingQueue {
        fn enqueue(
            &self,
            _channel: &str,
            _event: &NotificationEvent,
            _now_unix_ms: u64,
        ) -> Result<u64, QueueError> {
            *self.count.lock().unwrap() += 1;
            Ok(1)
        }
        fn claim_due(
            &self,
            _now_unix_ms: u64,
            _limit: u32,
            _lease_ms: u64,
        ) -> Result<Vec<QueuedNotification>, QueueError> {
            Ok(Vec::new())
        }
        fn mark_delivered(&self, _id: u64, _at: u64) -> Result<(), QueueError> {
            Ok(())
        }
        fn mark_retry(
            &self,
            _id: u64,
            _next: u64,
            _error: &str,
            _now: u64,
        ) -> Result<(), QueueError> {
            Ok(())
        }
        fn mark_dead(&self, _id: u64, _error: &str, _now: u64) -> Result<(), QueueError> {
            Ok(())
        }
    }

    fn config() -> NotificationConfig {
        // A webhook channel so dispatch ENQUEUES to the queue (the local-log
        // channel is written inline by dispatch and never touches the queue).
        let mut ssrf = custos_config::SsrfPolicy::default_blocking();
        ssrf.block_loopback = false;
        NotificationConfig {
            rules: vec![custos_config::NotificationRule {
                id: "r1".into(),
                event: None,
                severity_min: Severity::Info,
                resource: None,
                channels: vec!["wh".into()],
                log: false,
                dedup_window_ms: 0,
            }],
            channels: [(
                "wh".into(),
                ChannelConfig::Webhook(custos_config::WebhookConfig {
                    url: "http://127.0.0.1:9/hook".into(),
                    timeout_ms: 200,
                    retries: 0,
                    backoff_ms: 1,
                    headers: Default::default(),
                    hmac_secret: None,
                    tls_verify: true,
                    ssrf,
                }),
            )]
            .into(),
        }
    }

    fn event(n: usize) -> NotificationEvent {
        NotificationEvent::new(
            EventId::new(format!("evt-{n}")),
            EventType::ResourceUnhealthy,
            Severity::Critical,
            "summary",
            1_000,
        )
        .with_resource_id(ResourceId::new("web"))
    }

    fn wait_for(queue: &CountingQueue, want: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let got = *queue.count.lock().unwrap();
            if got >= want || Instant::now() > deadline {
                return got;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn spool_hands_events_to_the_worker_and_drains() {
        // C18: dispatch must not run the SQLite insert on the caller's
        // thread — the event is handed to the worker and enqueued there.
        let queue = Arc::new(CountingQueue {
            count: Mutex::new(0),
        });
        let engine = Arc::new(NotificationEngine::new(config(), queue.clone()));
        let spool = SpooledDispatch::spawn(engine, 16);
        for n in 0..8 {
            spool.dispatch(&event(n));
        }
        let got = wait_for(&queue, 8);
        assert_eq!(got, 8, "every spooled event is enqueued by the worker");
    }

    #[test]
    fn saturated_spool_falls_back_without_losing_events() {
        // A rendezvous spool (capacity 0) hands off only when the worker is
        // parked in recv; every other event takes the inline fallback. The
        // contract: no event is dropped, whichever path each one took.
        let queue = Arc::new(CountingQueue {
            count: Mutex::new(0),
        });
        let engine = Arc::new(NotificationEngine::new(config(), queue.clone()));
        let spool = SpooledDispatch::spawn(engine, 0);
        for n in 0..20 {
            spool.dispatch(&event(n));
        }
        let got = wait_for(&queue, 20);
        assert_eq!(got, 20, "a saturated spool must not drop events");
    }
}

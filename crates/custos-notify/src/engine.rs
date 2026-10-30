//! The notification engine (docs/guide/notification.md §8.1, §8.6).
//!
//! Two responsibilities, both non-blocking for recovery:
//!
//! 1. **Dispatch** — an event from the core engine is matched against the
//!    independent `[notification]` rules and enqueued to each selected
//!    channel's queue entry. The local structured log (base audit output,
//!    §8.2) is written immediately and can never fail.
//! 2. **Deliver** — a worker claims due queue entries and sends them through
//!    the configured channel; success → `delivered`, transient failure →
//!    `retry_scheduled` with exponential backoff, terminal failure →
//!    `dead_letter`.
//!
//! Enqueue and delivery never block recovery: dispatch is a fast database
//! insert, and delivery failures are recorded in the queue, not propagated.

use crate::channel::{Channel, ChannelError, ChannelErrorKind};
use crate::local_log::LocalLogChannel;
use crate::queue::NotificationQueue;
use crate::smtp::SmtpChannel;
use crate::webhook::WebhookChannel;
use custos_config::{ChannelConfig, NotificationConfig};
use custos_model::NotificationEvent;
use std::collections::HashMap;

/// Base retry backoff; the queue engine doubles it per attempt, capped.
const BASE_RETRY_MS: u64 = 30_000;
const MAX_RETRY_MS: u64 = 3_600_000;

/// Result of one dispatch (enqueue) pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DispatchReport {
    /// Queue entries written (one per selected channel per matched rule).
    pub enqueued: u32,
    /// Entries whose enqueue failed (persistence error); logged, not thrown.
    pub failed: u32,
    /// Events written straight to the local structured log.
    pub local_logged: u32,
}

/// Result of one delivery pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeliverReport {
    pub delivered: u32,
    pub retried: u32,
    pub dead: u32,
    /// Delivery attempts that could not even be recorded (queue error).
    pub failed: u32,
}

/// The notification engine: rule matching, channel construction and the
/// enqueue/deliver split.
pub struct NotificationEngine<Q: NotificationQueue> {
    config: NotificationConfig,
    channels: HashMap<String, Box<dyn Channel>>,
    queue: Q,
}

impl<Q: NotificationQueue> NotificationEngine<Q> {
    /// Builds the channels from the validated config. A channel whose
    /// construction fails (e.g. an SSRF-blocked webhook target) is skipped
    /// with a log line rather than failing startup — rules referencing it
    /// will dead-letter instead (§8.1 never blocks).
    pub fn new(config: NotificationConfig, queue: Q) -> Self {
        let mut channels = HashMap::new();
        for (id, channel_config) in &config.channels {
            let built: Result<Box<dyn Channel>, ChannelError> = match channel_config {
                ChannelConfig::LocalLog { enabled } => {
                    Ok(Box::new(LocalLogChannel::new(id.clone(), *enabled)))
                }
                ChannelConfig::Webhook(cfg) => WebhookChannel::new(id.clone(), cfg.clone())
                    .map(|c| Box::new(c) as Box<dyn Channel>),
                ChannelConfig::Smtp(cfg) => SmtpChannel::new(id.clone(), cfg.clone())
                    .map(|c| Box::new(c) as Box<dyn Channel>),
            };
            match built {
                Ok(channel) => {
                    channels.insert(id.clone(), channel);
                }
                Err(err) => {
                    tracing::error!(channel = %id, error = %err, "notification channel failed to build; disabled");
                }
            }
        }
        Self {
            config,
            channels,
            queue,
        }
    }

    /// True if any enabled local-log channel exists (the base audit sink).
    pub fn has_local_log(&self) -> bool {
        self.config
            .channels
            .values()
            .any(|c| matches!(c, ChannelConfig::LocalLog { enabled: true }))
    }

    /// Matches the event against the rules and enqueues it to the selected
    /// channels. The local structured log is written first (base audit,
    /// always on). Never blocks recovery.
    pub fn dispatch(&self, event: &NotificationEvent, now_unix_ms: u64) -> DispatchReport {
        let mut report = DispatchReport::default();

        // Base audit: every event goes to every enabled local-log channel.
        for (id, config) in &self.config.channels {
            if matches!(config, ChannelConfig::LocalLog { enabled: true }) {
                if let Some(channel) = self.channels.get(id) {
                    if channel.deliver(event).is_ok() {
                        report.local_logged += 1;
                    }
                }
            }
        }

        for rule in &self.config.rules {
            if !rule_matches(rule, event) {
                continue;
            }
            for channel_id in &rule.channels {
                // Local-log channels were handled above.
                if matches!(
                    self.config.channels.get(channel_id),
                    Some(ChannelConfig::LocalLog { .. })
                ) {
                    continue;
                }
                match self.queue.enqueue(channel_id, event, now_unix_ms) {
                    Ok(_) => report.enqueued += 1,
                    Err(err) => {
                        report.failed += 1;
                        tracing::error!(
                            channel = %channel_id,
                            error = %err,
                            "notification enqueue failed"
                        );
                    }
                }
            }
        }
        report
    }

    /// One delivery pass: claim due entries, send through the channel, and
    /// record the outcome.
    pub fn deliver_due(&self, now_unix_ms: u64, limit: u32) -> DeliverReport {
        let mut report = DeliverReport::default();
        let claimed = match self.queue.claim_due(now_unix_ms, limit) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(error = %err, "notification claim failed");
                return report;
            }
        };
        for row in claimed {
            let outcome = match self.channels.get(&row.channel) {
                Some(channel) => channel.deliver(&row.event),
                None => Err(ChannelError::new(
                    ChannelErrorKind::Config,
                    format!("unknown channel `{}`", row.channel),
                )),
            };
            let record = match &outcome {
                Ok(()) => self.queue.mark_delivered(row.id, now_unix_ms),
                Err(err)
                    if matches!(
                        err.kind,
                        ChannelErrorKind::Rejected | ChannelErrorKind::Config
                    ) =>
                {
                    self.queue.mark_dead(row.id, &err.to_string(), now_unix_ms)
                }
                Err(err) => {
                    let next = retry_at(now_unix_ms, row.attempts);
                    self.queue.mark_retry(row.id, next, &err.to_string())
                }
            };
            match record {
                Ok(()) => match &outcome {
                    Ok(()) => report.delivered += 1,
                    Err(err)
                        if matches!(
                            err.kind,
                            ChannelErrorKind::Rejected | ChannelErrorKind::Config
                        ) =>
                    {
                        report.dead += 1;
                    }
                    Err(_) => report.retried += 1,
                },
                Err(err) => {
                    report.failed += 1;
                    tracing::error!(id = row.id, error = %err, "notification outcome not recorded");
                }
            }
        }
        report
    }
}

/// True when the rule's filters accept the event.
fn rule_matches(rule: &custos_config::NotificationRule, event: &NotificationEvent) -> bool {
    if let Some(expected) = rule.event {
        if expected != event.event_type {
            return false;
        }
    }
    if event.severity < rule.severity_min {
        return false;
    }
    if let Some(resource) = &rule.resource {
        if event.resource_id.as_ref() != Some(resource) {
            return false;
        }
    }
    true
}

/// Exponential backoff for the next queue-level attempt, capped. `attempts`
/// is the count of prior attempts; the shift is capped so the delay always
/// reaches the cap rather than growing without bound.
fn retry_at(now_unix_ms: u64, attempts: u32) -> u64 {
    let shift = attempts.min(8);
    let delay = (BASE_RETRY_MS << shift).min(MAX_RETRY_MS);
    now_unix_ms.saturating_add(delay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{NotificationQueue, QueueError, QueuedNotification};
    use custos_config::{NotificationRule, SsrfPolicy, WebhookConfig};
    use custos_model::{EventId, EventType, NotificationState, ResourceId, Severity};

    /// In-memory queue port recording operations for assertions. Uses
    /// `RefCell` because the trait takes `&self` (like the real store).
    struct MemQueue {
        rows: std::cell::RefCell<Vec<QueuedNotification>>,
        next_id: std::cell::Cell<u64>,
        ops: std::cell::RefCell<Vec<String>>,
    }

    impl Default for MemQueue {
        fn default() -> Self {
            Self {
                rows: Default::default(),
                next_id: std::cell::Cell::new(0),
                ops: Default::default(),
            }
        }
    }

    impl MemQueue {
        fn ops(&self) -> Vec<String> {
            self.ops.borrow().clone()
        }
    }

    impl NotificationQueue for MemQueue {
        fn enqueue(
            &self,
            channel: &str,
            event: &NotificationEvent,
            _now_unix_ms: u64,
        ) -> Result<u64, QueueError> {
            let id = self.next_id.get() + 1;
            self.next_id.set(id);
            self.ops
                .borrow_mut()
                .push(format!("enqueue:{channel}:{}", event.event_id));
            self.rows.borrow_mut().push(QueuedNotification {
                id,
                channel: channel.into(),
                event: event.clone(),
                state: NotificationState::Pending,
                attempts: 0,
                last_error: None,
            });
            Ok(id)
        }

        fn claim_due(
            &self,
            _now_unix_ms: u64,
            limit: u32,
        ) -> Result<Vec<QueuedNotification>, QueueError> {
            let mut claimed = Vec::new();
            for row in self.rows.borrow_mut().iter_mut().take(limit as usize) {
                if row.state == NotificationState::Pending {
                    row.state = NotificationState::Sending;
                    claimed.push(row.clone());
                }
            }
            Ok(claimed)
        }

        fn mark_delivered(&self, id: u64, _at: u64) -> Result<(), QueueError> {
            self.ops.borrow_mut().push(format!("delivered:{id}"));
            for r in self.rows.borrow_mut().iter_mut() {
                if r.id == id {
                    r.state = NotificationState::Delivered;
                }
            }
            Ok(())
        }

        fn mark_retry(&self, id: u64, _next: u64, error: &str) -> Result<(), QueueError> {
            self.ops.borrow_mut().push(format!("retry:{id}:{error}"));
            for r in self.rows.borrow_mut().iter_mut() {
                if r.id == id {
                    r.state = NotificationState::RetryScheduled;
                    r.attempts += 1;
                    r.last_error = Some(error.into());
                }
            }
            Ok(())
        }

        fn mark_dead(&self, id: u64, error: &str, _now: u64) -> Result<(), QueueError> {
            self.ops.borrow_mut().push(format!("dead:{id}:{error}"));
            for r in self.rows.borrow_mut().iter_mut() {
                if r.id == id {
                    r.state = NotificationState::DeadLetter;
                    r.last_error = Some(error.into());
                }
            }
            Ok(())
        }
    }

    fn event(event_type: EventType, severity: Severity) -> NotificationEvent {
        NotificationEvent::new(EventId::new("evt"), event_type, severity, "summary", 1_000)
            .with_resource_id(ResourceId::new("web"))
    }

    fn rule(
        id: &str,
        event: Option<EventType>,
        severity_min: Severity,
        resource: Option<&str>,
        channels: Vec<&str>,
    ) -> NotificationRule {
        NotificationRule {
            id: id.into(),
            event,
            severity_min,
            resource: resource.map(ResourceId::new),
            channels: channels.into_iter().map(str::to_owned).collect(),
            log: false,
        }
    }

    fn webhook_channel_config() -> ChannelConfig {
        let mut ssrf = SsrfPolicy::default_blocking();
        ssrf.block_loopback = false;
        ChannelConfig::Webhook(WebhookConfig {
            url: "http://127.0.0.1:9/hook".into(),
            timeout_ms: 200,
            retries: 0,
            backoff_ms: 1,
            headers: Default::default(),
            hmac_secret: None,
            tls_verify: true,
            ssrf,
        })
    }

    #[test]
    fn dispatch_matches_rule_and_enqueues() {
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![rule(
                "r1",
                Some(EventType::ResourceUnhealthy),
                Severity::Warning,
                Some("web"),
                vec!["wh"],
            )],
            channels: [("wh".into(), webhook_channel_config())].into(),
        };
        let engine = NotificationEngine::new(config, queue);

        // Matching event: enqueued to "wh".
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            1_000,
        );
        assert_eq!(report.enqueued, 1);
        assert!(engine
            .queue
            .ops()
            .iter()
            .any(|o| o.starts_with("enqueue:wh:")));

        // Non-matching event type: not enqueued.
        let report = engine.dispatch(
            &event(EventType::ResourceDegraded, Severity::Critical),
            1_000,
        );
        assert_eq!(report.enqueued, 0);
    }

    #[test]
    fn dispatch_filters_by_severity_and_resource() {
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![rule(
                "r1",
                None,
                Severity::Critical,
                Some("web"),
                vec!["wh"],
            )],
            channels: [("wh".into(), webhook_channel_config())].into(),
        };
        let engine = NotificationEngine::new(config, queue);

        // Below severity threshold: not matched.
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Warning),
            1_000,
        );
        assert_eq!(report.enqueued, 0);

        // Wrong resource: not matched.
        let wrong = event(EventType::ResourceUnhealthy, Severity::Critical)
            .with_resource_id(ResourceId::new("db"));
        let report = engine.dispatch(&wrong, 1_000);
        assert_eq!(report.enqueued, 0);

        // Match.
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            1_000,
        );
        assert_eq!(report.enqueued, 1);
    }

    #[test]
    fn local_log_is_written_for_every_event() {
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![],
            channels: [("audit".into(), ChannelConfig::LocalLog { enabled: true })].into(),
        };
        let engine = NotificationEngine::new(config, queue);
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            1_000,
        );
        assert_eq!(report.local_logged, 1);
        assert_eq!(report.enqueued, 0);
    }

    #[test]
    fn deliver_due_sends_and_records() {
        let queue = MemQueue::default();
        // A channel that always succeeds: reuse the local-log channel (never
        // fails) but enqueue a row for it directly.
        let config = NotificationConfig {
            rules: vec![],
            channels: [("audit".into(), ChannelConfig::LocalLog { enabled: true })].into(),
        };
        let engine = NotificationEngine::new(config, queue);
        // Manually enqueue a row (bypass dispatch, which handles local-log).
        let event = event(EventType::ResourceUnhealthy, Severity::Critical);
        engine.queue.enqueue("audit", &event, 1_000).unwrap();

        let report = engine.deliver_due(1_000, 10);
        assert_eq!(report.delivered, 1);
        assert_eq!(report.retried, 0);
        assert_eq!(report.dead, 0);
        assert!(engine
            .queue
            .ops()
            .iter()
            .any(|o| o.starts_with("delivered:")));
    }

    #[test]
    fn deliver_due_dead_letters_terminal_and_retries_transient() {
        // A channel whose construction is fine but which will fail on send:
        // use a refused-port webhook (transport, retryable) and an unknown
        // channel (config, terminal).
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![],
            channels: [("wh".into(), webhook_channel_config())].into(),
        };
        let engine = NotificationEngine::new(config, queue);

        // Row for the refused-port channel: transport → retry.
        let event = event(EventType::ResourceUnhealthy, Severity::Critical);
        let id = engine.queue.enqueue("wh", &event, 1_000).unwrap();
        let report = engine.deliver_due(1_000, 10);
        assert_eq!(report.retried, 1);
        assert_eq!(report.delivered, 0);
        assert!(engine
            .queue
            .ops()
            .iter()
            .any(|o| o.starts_with(&format!("retry:{id}:"))));

        // Row for a nonexistent channel: config → dead.
        let id2 = engine.queue.enqueue("ghost", &event, 1_000).unwrap();
        let report = engine.deliver_due(1_000, 10);
        assert_eq!(report.dead, 1);
        assert!(engine
            .queue
            .ops()
            .iter()
            .any(|o| o.starts_with(&format!("dead:{id2}:"))));
    }

    #[test]
    fn retry_backoff_doubles_and_caps() {
        assert_eq!(retry_at(1_000, 0), 31_000);
        assert_eq!(retry_at(1_000, 1), 61_000);
        // 30_000 << 7 = 3_840_000 exceeds the 1 h cap.
        assert_eq!(retry_at(1_000, 7), 1_000 + 3_600_000);
        assert_eq!(retry_at(1_000, 20), 1_000 + 3_600_000);
    }
}

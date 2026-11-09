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
use custos_model::{EventType, NotificationEvent};
use std::collections::HashMap;
use std::sync::Mutex;

/// Base retry backoff; the queue engine doubles it per attempt, capped.
const BASE_RETRY_MS: u64 = 30_000;
const MAX_RETRY_MS: u64 = 3_600_000;
/// Queue-level attempt cap (C16/C17): a transient failure past this many
/// prior attempts dead-letters instead of scheduling yet another retry.
/// Keeps the attempts counter and the retry schedule bounded; a channel
/// down past the cap surfaces as DeadLetter to the human. 8 aligns with
/// the backoff ladder, which saturates at this many shifts (~2 h of
/// backoff before the terminal state).
const MAX_QUEUE_ATTEMPTS: u32 = 8;

/// Result of one dispatch (enqueue) pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DispatchReport {
    /// Queue entries written (one per selected channel per matched rule).
    pub enqueued: u32,
    /// Entries whose enqueue failed (persistence error); logged, not thrown.
    pub failed: u32,
    /// Events written straight to the local structured log.
    pub local_logged: u32,
    /// Enqueues suppressed by the same-fault dedup window (§8.5).
    pub suppressed: u32,
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

/// The transition one claim pass records; drives the report counters so a
/// dead-lettered row is counted as `dead` regardless of which arm produced
/// the terminal state (config/rejected vs retry-ladder exhaustion).
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutcomeTransition {
    Delivered,
    Retried,
    Dead,
}

/// Same-fault dedup state (docs/guide/notification.md §8.5 聚合并抑制重复通知):
/// a fault key is suppressed until its window expires; repeats during the
/// window are counted (and that count is logged), and a recovery clears the
/// resource's keys so the next fault notifies promptly.
#[derive(Default)]
struct DedupState {
    /// Fault key → suppression expires at (unix ms).
    suppressed_until: HashMap<String, u64>,
    /// Fault key → repeats suppressed since the last notification.
    repeats: HashMap<String, u32>,
}

/// The notification engine: rule matching, channel construction and the
/// enqueue/deliver split.
pub struct NotificationEngine<Q: NotificationQueue> {
    config: NotificationConfig,
    channels: HashMap<String, Box<dyn Channel>>,
    queue: Q,
    dedup: Mutex<DedupState>,
    /// Secrets collected from the channel configs; every error string that
    /// is persisted to the queue or logged passes through it (§8.7).
    scrubber: crate::scrub::Scrubber,
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
                ChannelConfig::Herald(cfg) => {
                    crate::herald::HeraldChannel::new(id.clone(), cfg.clone())
                        .map(|c| Box::new(c) as Box<dyn Channel>)
                }
                ChannelConfig::Telegram(cfg) => Ok(Box::new(crate::telegram::TelegramChannel::new(
                    id.clone(),
                    cfg.bot_token.expose(),
                    cfg.chat_id.clone(),
                    cfg.timeout_ms,
                )) as Box<dyn Channel>),
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
            scrubber: crate::scrub::Scrubber::from_config(&config),
            config,
            channels,
            queue,
            dedup: Mutex::new(DedupState::default()),
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
    /// always on). Repeats of the same fault within a rule's dedup window
    /// are suppressed and counted (§8.5). Never blocks recovery.
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

        // `notification.delivery_failed` is loop-suppressed (§8.4/§8.5): a
        // remote delivery of THIS event failing would emit another
        // delivery_failed, which would fail again — an infinite
        // self-amplification loop. It therefore never enqueues to remote
        // channels; the local structured log above is its only sink.
        if event.event_type == EventType::NotificationDeliveryFailed {
            return report;
        }

        // A verified recovery clears the resource's fault keys so the next
        // fault notifies promptly instead of waiting out the window.
        if event.event_type == EventType::RecoverySucceeded {
            let mut dedup = self.dedup.lock().unwrap_or_else(|e| e.into_inner());
            clear_resource_keys(&mut dedup, event);
        }

        for rule in &self.config.rules {
            if !rule_matches(rule, event) {
                continue;
            }
            // Same-fault dedup: suppress repeats within the window.
            if rule.dedup_window_ms > 0 {
                let key = fault_key(event);
                let mut dedup = self.dedup.lock().unwrap_or_else(|e| e.into_inner());
                let until = dedup.suppressed_until.get(&key).copied();
                if let Some(until) = until {
                    if now_unix_ms < until {
                        let repeats = dedup.repeats.entry(key.clone()).or_insert(0);
                        *repeats += 1;
                        report.suppressed += rule.channels.len() as u32;
                        tracing::debug!(
                            key = %key,
                            repeats = *repeats,
                            rule = %rule.id,
                            "notification suppressed (same fault within dedup window)"
                        );
                        continue;
                    }
                }
                dedup
                    .suppressed_until
                    .insert(key.clone(), now_unix_ms + rule.dedup_window_ms);
                if let Some(repeats) = dedup.repeats.remove(&key) {
                    if repeats > 0 {
                        tracing::info!(key = %key, repeats, "fault re-notified after suppressed repeats");
                    }
                }
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
    /// record the outcome. The claim lease is sized to outlive the longest
    /// legitimate delivery across the configured channels, so a live row is
    /// never reclaimed mid-delivery and sent concurrently (C7).
    pub fn deliver_due(&self, now_unix_ms: u64, limit: u32) -> DeliverReport {
        let mut report = DeliverReport::default();
        let lease_ms = delivery_lease_ms(&self.config);
        let claimed = match self.queue.claim_due(now_unix_ms, limit, lease_ms) {
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
            // Error strings are persisted as queue `last_error` (and would be
            // logged below): scrub configured secrets out first (§8.7).
            let (transition, record) = match &outcome {
                Ok(()) => (
                    OutcomeTransition::Delivered,
                    self.queue.mark_delivered(row.id, now_unix_ms),
                ),
                Err(err)
                    if matches!(
                        err.kind,
                        ChannelErrorKind::Rejected | ChannelErrorKind::Config
                    ) =>
                {
                    let scrubbed = self.scrubber.scrub(&err.to_string());
                    (
                        OutcomeTransition::Dead,
                        self.queue.mark_dead(row.id, &scrubbed, now_unix_ms),
                    )
                }
                Err(err) if row.attempts >= MAX_QUEUE_ATTEMPTS => {
                    // The queue-level retry ladder is exhausted (C16/C17): a
                    // channel down past the cap dead-letters instead of
                    // retrying forever. The attempts counter and the retry
                    // schedule stay bounded; the terminal DeadLetter state is
                    // surfaced to the human (notify status counts) rather
                    // than churning silently.
                    let scrubbed = self.scrubber.scrub(&err.to_string());
                    (
                        OutcomeTransition::Dead,
                        self.queue.mark_dead(row.id, &scrubbed, now_unix_ms),
                    )
                }
                Err(err) => {
                    let next = retry_at(now_unix_ms, row.attempts);
                    let scrubbed = self.scrubber.scrub(&err.to_string());
                    (
                        OutcomeTransition::Retried,
                        self.queue.mark_retry(row.id, next, &scrubbed, now_unix_ms),
                    )
                }
            };
            match record {
                Ok(()) => match transition {
                    OutcomeTransition::Delivered => report.delivered += 1,
                    OutcomeTransition::Retried => report.retried += 1,
                    OutcomeTransition::Dead => report.dead += 1,
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

/// Claim-lease floor: the pre-lease staleness window (§8.6 crash recovery
/// latency; also covers any sane single-attempt delivery, C19).
const DEFAULT_LEASE_MS: u64 = 60_000;
/// Claim-lease cap: bounds the crash-recovery reclaim latency even for an
/// absurd timeout configuration.
const MAX_LEASE_MS: u64 = 3_600_000;

/// One channel's delivery upper bound (C7): the claim lease must outlive
/// the longest legitimate delivery across the configured channels, so a
/// live delivery is never reclaimed and sent concurrently. Every remote
/// channel makes one attempt per deliver (C19/C22: the queue owns
/// retries), so each budget is that channel's own send timeout.
fn channel_delivery_budget_ms(config: &ChannelConfig) -> u64 {
    match config {
        ChannelConfig::LocalLog { .. } => 0,
        ChannelConfig::Smtp(cfg) => cfg.connect_timeout_ms.saturating_add(cfg.send_timeout_ms),
        ChannelConfig::Webhook(cfg) => cfg.timeout_ms,
        ChannelConfig::Herald(cfg) => cfg.timeout_ms,
        ChannelConfig::Telegram(cfg) => cfg.timeout_ms,
    }
}

/// The lease for one claim batch: the longest configured channel's
/// delivery bound, floored at the staleness window and capped.
fn delivery_lease_ms(config: &NotificationConfig) -> u64 {
    let longest = config
        .channels
        .values()
        .map(channel_delivery_budget_ms)
        .max()
        .unwrap_or(0);
    longest.clamp(DEFAULT_LEASE_MS, MAX_LEASE_MS)
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

/// Stable same-fault key: `{event_type}:{resource}` (or `-` when the event
/// has no resource). One key per fault class per resource.
fn fault_key(event: &NotificationEvent) -> String {
    let resource = event
        .resource_id
        .as_ref()
        .map(|r| r.as_str())
        .unwrap_or("-");
    format!("{}:{}", event.event_type.as_str(), resource)
}

/// Clears every dedup key belonging to the recovered resource, so the next
/// fault on it notifies immediately instead of waiting out the window.
fn clear_resource_keys(dedup: &mut DedupState, event: &NotificationEvent) {
    let Some(resource) = event.resource_id.as_ref() else {
        return;
    };
    let prefix = format!(":{}", resource.as_str());
    dedup
        .suppressed_until
        .retain(|key, _| !key.ends_with(&prefix));
    dedup.repeats.retain(|key, _| !key.ends_with(&prefix));
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
            _lease_ms: u64,
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

        fn mark_retry(
            &self,
            id: u64,
            _next: u64,
            error: &str,
            _now: u64,
        ) -> Result<(), QueueError> {
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
            dedup_window_ms: 0,
        }
    }

    fn dedup_rule(
        id: &str,
        event: Option<EventType>,
        severity_min: Severity,
        resource: Option<&str>,
        channels: Vec<&str>,
        dedup_window_ms: u64,
    ) -> NotificationRule {
        let mut rule = rule(id, event, severity_min, resource, channels);
        rule.dedup_window_ms = dedup_window_ms;
        rule
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
    fn transient_failures_dead_letter_at_the_attempt_cap() {
        // C16/C17: a transient failure always called mark_retry with no
        // attempt cap — a permanently-down channel retried forever, its
        // attempts counter and retry schedule growing without bound. At the
        // cap the row must dead-letter (terminal, surfaced to the human)
        // instead of scheduling a retry.
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![],
            channels: [("wh".into(), webhook_channel_config())].into(),
        };
        let engine = NotificationEngine::new(config, queue);
        let event = event(EventType::ResourceUnhealthy, Severity::Critical);
        let id = engine.queue.enqueue("wh", &event, 1_000).unwrap();

        // At the cap: the failure must dead-letter, not schedule retry N+1.
        {
            let mut rows = engine.queue.rows.borrow_mut();
            let row = rows.iter_mut().find(|r| r.id == id).unwrap();
            row.state = NotificationState::Pending;
            row.attempts = MAX_QUEUE_ATTEMPTS;
        }
        let report = engine.deliver_due(1_000, 10);
        assert_eq!(report.dead, 1, "a channel down past the cap dead-letters");
        assert!(engine
            .queue
            .ops()
            .iter()
            .any(|o| o.starts_with(&format!("dead:{id}:"))));

        // One under the cap: still retried.
        {
            let mut rows = engine.queue.rows.borrow_mut();
            let row = rows.iter_mut().find(|r| r.id == id).unwrap();
            row.state = NotificationState::Pending;
            row.attempts = MAX_QUEUE_ATTEMPTS - 1;
        }
        let report = engine.deliver_due(2_000, 10);
        assert_eq!(
            report.retried, 1,
            "under the cap the transient failure retries"
        );
        assert!(engine
            .queue
            .ops()
            .iter()
            .any(|o| o.starts_with(&format!("retry:{id}:"))));
    }

    #[test]
    fn retry_backoff_doubles_and_caps() {
        assert_eq!(retry_at(1_000, 0), 31_000);
        assert_eq!(retry_at(1_000, 1), 61_000);
        // 30_000 << 7 = 3_840_000 exceeds the 1 h cap.
        assert_eq!(retry_at(1_000, 7), 1_000 + 3_600_000);
        assert_eq!(retry_at(1_000, 20), 1_000 + 3_600_000);
    }

    #[test]
    fn dedup_suppresses_repeats_within_window_then_renotifies() {
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![dedup_rule(
                "r1",
                Some(EventType::ResourceUnhealthy),
                Severity::Info,
                Some("web"),
                vec!["wh"],
                10_000,
            )],
            channels: [("wh".into(), webhook_channel_config())].into(),
        };
        let engine = NotificationEngine::new(config, queue);

        // First occurrence notifies.
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            1_000,
        );
        assert_eq!(report.enqueued, 1);
        assert_eq!(report.suppressed, 0);

        // Repeat within the window is suppressed and counted.
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            2_000,
        );
        assert_eq!(report.enqueued, 0);
        assert_eq!(report.suppressed, 1);

        // After the window expires the fault notifies again.
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            12_000,
        );
        assert_eq!(report.enqueued, 1);
        assert_eq!(report.suppressed, 0);
    }

    #[test]
    fn dedup_keys_differ_per_resource_and_zero_window_disables() {
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![
                dedup_rule(
                    "r1",
                    Some(EventType::ResourceUnhealthy),
                    Severity::Info,
                    Some("web"),
                    vec!["wh"],
                    10_000,
                ),
                dedup_rule(
                    "r2",
                    Some(EventType::ResourceUnhealthy),
                    Severity::Info,
                    Some("db"),
                    vec!["wh"],
                    0,
                ),
            ],
            channels: [("wh".into(), webhook_channel_config())].into(),
        };
        let engine = NotificationEngine::new(config, queue);

        // Same event type, different resources: independent keys.
        let web = event(EventType::ResourceUnhealthy, Severity::Critical);
        let db = event(EventType::ResourceUnhealthy, Severity::Critical)
            .with_resource_id(ResourceId::new("db"));
        engine.dispatch(&web, 1_000);
        let report = engine.dispatch(&db, 1_000);
        assert_eq!(report.enqueued, 1, "db rule has dedup disabled");

        // Repeat for db (dedup disabled) still notifies; web is suppressed.
        let report = engine.dispatch(&db, 2_000);
        assert_eq!(report.enqueued, 1);
        let report = engine.dispatch(&web, 2_000);
        assert_eq!(report.enqueued, 0);
        assert_eq!(report.suppressed, 1);
    }

    #[test]
    fn persisted_errors_carry_no_configured_secrets() {
        // A webhook URL embedding userinfo credentials + an HMAC secret: the
        // channel fails (refused port) and whatever error text the transport
        // produced is scrubbed before it reaches the queue.
        let queue = MemQueue::default();
        let mut ssrf = SsrfPolicy::default_blocking();
        ssrf.block_loopback = false;
        let config = NotificationConfig {
            rules: vec![rule(
                "r1",
                Some(EventType::ResourceUnhealthy),
                Severity::Info,
                Some("web"),
                vec!["wh"],
            )],
            channels: [(
                "wh".into(),
                ChannelConfig::Webhook(WebhookConfig {
                    url: "http://deploy:leaky-url-pass@127.0.0.1:9/hook".into(),
                    timeout_ms: 200,
                    retries: 0,
                    backoff_ms: 1,
                    headers: Default::default(),
                    hmac_secret: Some(custos_config::SecretString::new("leaky-hmac")),
                    tls_verify: true,
                    ssrf,
                }),
            )]
            .into(),
        };
        let engine = NotificationEngine::new(config, queue);
        engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            1_000,
        );
        engine.deliver_due(1_000, 10);

        let ops = engine.queue.ops();
        for op in ops {
            assert!(!op.contains("leaky-url-pass"), "{op}");
            assert!(!op.contains("leaky-hmac"), "{op}");
        }
    }

    #[test]
    fn delivery_failed_never_enqueues_to_remote_channels() {
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![rule(
                "r1",
                Some(EventType::NotificationDeliveryFailed),
                Severity::Info,
                None,
                vec!["wh"],
            )],
            channels: [
                ("wh".into(), webhook_channel_config()),
                ("audit".into(), ChannelConfig::LocalLog { enabled: true }),
            ]
            .into(),
        };
        let engine = NotificationEngine::new(config, queue);

        // A rule explicitly selecting a remote channel still cannot make
        // delivery_failed enqueue: loop suppression wins over rules.
        let failed = NotificationEvent::new(
            EventId::new("evt-f"),
            EventType::NotificationDeliveryFailed,
            Severity::Warning,
            "webhook transport error",
            1_000,
        );
        let report = engine.dispatch(&failed, 1_000);
        assert_eq!(report.enqueued, 0, "delivery_failed must not enqueue");
        assert_eq!(report.suppressed, 0);
        // It still lands in the local structured log.
        assert_eq!(report.local_logged, 1);
    }

    #[test]
    fn recovery_clears_dedup_keys_for_the_resource() {
        let queue = MemQueue::default();
        let config = NotificationConfig {
            rules: vec![
                dedup_rule(
                    "r1",
                    Some(EventType::ResourceUnhealthy),
                    Severity::Info,
                    Some("web"),
                    vec!["wh"],
                    10_000,
                ),
                dedup_rule(
                    "r2",
                    Some(EventType::RecoverySucceeded),
                    Severity::Info,
                    Some("web"),
                    vec!["wh"],
                    10_000,
                ),
            ],
            channels: [("wh".into(), webhook_channel_config())].into(),
        };
        let engine = NotificationEngine::new(config, queue);

        // Fault fires, then repeats are suppressed.
        engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            1_000,
        );
        assert_eq!(
            engine
                .dispatch(
                    &event(EventType::ResourceUnhealthy, Severity::Critical),
                    2_000
                )
                .suppressed,
            1
        );
        // Verified recovery clears the resource's keys.
        let recovered = event(EventType::RecoverySucceeded, Severity::Info);
        engine.dispatch(&recovered, 3_000);
        // The next fault on the same resource notifies immediately.
        let report = engine.dispatch(
            &event(EventType::ResourceUnhealthy, Severity::Critical),
            4_000,
        );
        assert_eq!(report.enqueued, 1);
        assert_eq!(report.suppressed, 0);
    }

    #[test]
    fn delivery_lease_floors_and_caps_per_attempt() {
        // C19/C22: channels make one attempt per deliver, so the budget is
        // the send timeout — a webhook with a 10s timeout sits at the
        // staleness-window floor.
        let mut ssrf = SsrfPolicy::default_blocking();
        ssrf.block_loopback = false;
        let webhook = ChannelConfig::Webhook(WebhookConfig {
            url: "http://127.0.0.1:9/hook".into(),
            timeout_ms: 10_000,
            retries: 6,
            backoff_ms: 30_000,
            headers: Default::default(),
            hmac_secret: None,
            tls_verify: true,
            ssrf,
        });
        assert_eq!(channel_delivery_budget_ms(&webhook), 10_000);
        let webhook_config = NotificationConfig {
            rules: Vec::new(),
            channels: [("wh".into(), webhook)].into(),
        };
        assert_eq!(delivery_lease_ms(&webhook_config), 60_000);

        // Local-log-only: no remote budget, so the lease sits at the
        // staleness-window floor.
        let local_only = NotificationConfig {
            rules: Vec::new(),
            channels: [("log".into(), ChannelConfig::LocalLog { enabled: true })].into(),
        };
        assert_eq!(delivery_lease_ms(&local_only), 60_000);

        // An absurd timeout is capped so crash-recovery latency stays
        // bounded.
        let mut ssrf = SsrfPolicy::default_blocking();
        ssrf.block_loopback = false;
        let absurd = ChannelConfig::Webhook(WebhookConfig {
            url: "http://127.0.0.1:9/hook".into(),
            timeout_ms: 10_000_000_000,
            retries: 0,
            backoff_ms: 1,
            headers: Default::default(),
            hmac_secret: None,
            tls_verify: true,
            ssrf,
        });
        assert!(channel_delivery_budget_ms(&absurd) > MAX_LEASE_MS);
        let absurd_config = NotificationConfig {
            rules: Vec::new(),
            channels: [("wh".into(), absurd)].into(),
        };
        assert_eq!(delivery_lease_ms(&absurd_config), 3_600_000);
    }
}

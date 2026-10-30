//! P1 acceptance gate (docs/guide/roadmap.md P1, todo.md 验收门):
//! 断网通知留队；恢复后续发；发送失败不影响恢复.
//!
//! The channel is a webhook pointing at a port that is initially refused
//! (simulated outage) and later rebinds (network back). Every entry
//! enqueued during the outage survives in the queue, dispatch never blocks,
//! and the first delivery pass after the network returns delivers it.

use custos_config::{
    ChannelConfig, NotificationConfig, NotificationRule, SsrfPolicy, WebhookConfig,
};
use custos_model::{
    EventId, EventType, NotificationEvent, NotificationState, ResourceId, Severity,
};
use custos_notify::{NotificationEngine, NotificationQueue, QueueError, QueuedNotification};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// In-memory queue that honors `next_attempt_at` like the real store:
/// an entry is claimable when its state is pending and it is not deferred.
#[derive(Clone)]
struct TimeAwareQueue {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    rows: Mutex<Vec<QueueRow>>,
    next_id: Mutex<u64>,
}

#[derive(Clone)]
struct QueueRow {
    id: u64,
    channel: String,
    event: NotificationEvent,
    state: NotificationState,
    attempts: u32,
    next_at: Option<u64>,
    last_error: Option<String>,
}

impl TimeAwareQueue {
    fn new() -> Self {
        Self {
            inner: Arc::new(QueueInner {
                rows: Mutex::new(Vec::new()),
                next_id: Mutex::new(0),
            }),
        }
    }

    fn state_of(&self, id: u64) -> NotificationState {
        self.inner
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.state)
            .expect("entry exists")
    }
}

impl NotificationQueue for TimeAwareQueue {
    fn enqueue(
        &self,
        channel: &str,
        event: &NotificationEvent,
        _now_unix_ms: u64,
    ) -> Result<u64, QueueError> {
        let mut next_id = self.inner.next_id.lock().unwrap();
        *next_id += 1;
        let id = *next_id;
        self.inner.rows.lock().unwrap().push(QueueRow {
            id,
            channel: channel.into(),
            event: event.clone(),
            state: NotificationState::Pending,
            attempts: 0,
            next_at: None,
            last_error: None,
        });
        Ok(id)
    }

    fn claim_due(
        &self,
        now_unix_ms: u64,
        limit: u32,
    ) -> Result<Vec<QueuedNotification>, QueueError> {
        let mut claimed = Vec::new();
        let mut rows = self.inner.rows.lock().unwrap();
        for row in rows.iter_mut() {
            if claimed.len() as u32 >= limit {
                break;
            }
            let due = matches!(
                row.state,
                NotificationState::Pending | NotificationState::RetryScheduled
            ) && row.next_at.is_none_or(|at| at <= now_unix_ms);
            if due {
                row.state = NotificationState::Sending;
                claimed.push(QueuedNotification {
                    id: row.id,
                    channel: row.channel.clone(),
                    event: row.event.clone(),
                    state: NotificationState::Sending,
                    attempts: row.attempts,
                    last_error: row.last_error.clone(),
                });
            }
        }
        Ok(claimed)
    }

    fn mark_delivered(&self, id: u64, _at: u64) -> Result<(), QueueError> {
        for row in self.inner.rows.lock().unwrap().iter_mut() {
            if row.id == id {
                row.state = NotificationState::Delivered;
            }
        }
        Ok(())
    }

    fn mark_retry(&self, id: u64, next: u64, error: &str) -> Result<(), QueueError> {
        for row in self.inner.rows.lock().unwrap().iter_mut() {
            if row.id == id {
                row.state = NotificationState::RetryScheduled;
                row.attempts += 1;
                row.next_at = Some(next);
                row.last_error = Some(error.into());
            }
        }
        Ok(())
    }

    fn mark_dead(&self, id: u64, error: &str, _now: u64) -> Result<(), QueueError> {
        for row in self.inner.rows.lock().unwrap().iter_mut() {
            if row.id == id {
                row.state = NotificationState::DeadLetter;
                row.last_error = Some(error.into());
            }
        }
        Ok(())
    }
}

fn event() -> NotificationEvent {
    NotificationEvent::new(
        EventId::new("evt-offline"),
        EventType::RecoveryExhausted,
        Severity::Critical,
        "all attempts exhausted",
        1_000,
    )
    .with_resource_id(ResourceId::new("db"))
}

fn webhook_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/hook")
}

fn config_for(url: String) -> NotificationConfig {
    let mut ssrf = SsrfPolicy::default_blocking();
    ssrf.block_loopback = false;
    let mut channels = BTreeMap::new();
    channels.insert(
        "wh".to_owned(),
        ChannelConfig::Webhook(WebhookConfig {
            url,
            timeout_ms: 1_500,
            retries: 0,
            backoff_ms: 30_000,
            headers: BTreeMap::new(),
            hmac_secret: None,
            tls_verify: true,
            ssrf,
        }),
    );
    NotificationConfig {
        rules: vec![NotificationRule {
            id: "exhausted".into(),
            event: Some(EventType::RecoveryExhausted),
            severity_min: Severity::Warning,
            resource: None,
            channels: vec!["wh".into()],
            log: false,
            dedup_window_ms: 0,
        }],
        channels,
    }
}

/// A one-shot HTTP server on the given (already open) listener; returns how
/// many requests were served.
fn serve_once(listener: &TcpListener) -> usize {
    match listener.set_nonblocking(false) {
        Ok(()) => {}
        Err(_) => return 0,
    }
    if let Ok((mut stream, _)) = listener.accept() {
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        1
    } else {
        0
    }
}

#[test]
fn outage_keeps_entries_queued_and_delivers_after_network_returns() {
    // Reserve a port, then drop the listener to simulate a dead upstream.
    let (port, _guard) = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Keep the listener in scope until here, then drop it.
        (port, Some(listener))
    };
    drop(_guard);

    let queue = TimeAwareQueue::new();
    let engine = NotificationEngine::new(config_for(webhook_url(port)), queue.clone());

    // 1. The outage does not block dispatch: the event is enqueued fast.
    let started = Instant::now();
    let report = engine.dispatch(&event(), 1_000);
    assert_eq!(report.enqueued, 1);
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "dispatch must not block on the outage"
    );

    // 2. A delivery pass during the outage fails transiently and schedules
    //    a retry — the entry stays queued, never dropped.
    let report = engine.deliver_due(1_000, 10);
    assert_eq!(
        report.retried, 1,
        "transient failure is scheduled, not lost"
    );
    let id = 1;
    assert_eq!(queue.state_of(id), NotificationState::RetryScheduled);

    // 3. Not yet due: the backoff window keeps it deferred.
    assert_eq!(engine.deliver_due(1_100, 10).retried, 0);

    // 4. Network returns: rebind the port and serve the deferred entry.
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("rebind the port");
    std::thread::spawn(move || serve_once(&listener));
    std::thread::sleep(Duration::from_millis(50));

    let report = engine.deliver_due(1_000 + 61_000, 10);
    assert_eq!(report.delivered, 1, "queued entry drains after the outage");
    assert_eq!(queue.state_of(id), NotificationState::Delivered);
}

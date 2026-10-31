---
title: Notification Engine
---

# 8. Notification Engine

## 8.1 Design goals

Notification is an independent subsystem.

The Core Engine only produces structured events; it never calls the Telegram, SMTP or other platform APIs directly.

The notification system is responsible for:

- Event filtering.
- Rule matching.
- Notification construction.
- Persisting the notification queue.
- Channel selection.
- Retry and backoff.
- Deduplication and aggregation.
- Delivery status.
- Catch-up delivery after recovery.

Notification delivery failure must not block a RecoveryRun.

## 8.2 Built-in channels

The first version ships with:

### Local Log

Structured local logs, serving as the baseline audit output.

### Webhook

Sends JSON events to a user-configured HTTP or HTTPS endpoint.

Requirements:

- Request timeout.
- Retry and backoff.
- Event ID.
- Configurable authentication headers.
- Optional HMAC signing.
- TLS verification.
- A controlled destination address policy.
- SSRF protection for private, loopback and redirect targets.
- Never log secrets.
- Clearly distinguish an HTTP request failure from the peer's business-level confirmation failure.

The Webhook should be the first general-purpose external channel implemented.

### SMTP Email

Supports conventional SMTP email alerting.

Requirements:

- Connection and send timeouts.
- TLS support.
- Secure credential reading.
- Configurable sender and recipients.
- Retry on send failure.
- Explicit send results.

### Console

The CLI displays events in real time, for local debugging and runtime observation.

Console is not a remote notification channel.

## 8.3 Third-party notification plugins

Support for:

- Telegram.
- Discord.
- Slack.
- WeCom (Enterprise WeChat).
- DingTalk.
- Feishu (Lark).
- ntfy.
- Gotify.
- User-defined notification channels.

These channels plug in through a unified NotificationChannel interface; platform-specific logic must not be added to the Core Engine.

The first version does not need all of them built in. Prioritize making Webhook and SMTP usable.

## 8.4 Event model

```rust
struct NotificationEvent {
    event_id: EventId,
    event_type: EventType,
    severity: Severity,
    resource_id: Option<ResourceId>,
    incident_id: Option<IncidentId>,
    recovery_run_id: Option<RecoveryRunId>,
    occurred_at: DateTime,
    summary: String,
    details: SanitizedDetails,
}
```

Main event types:

```text
resource.unhealthy
resource.degraded
recovery.started
recovery.action_started
recovery.action_failed
recovery.succeeded
recovery.exhausted
recovery.outcome_unknown
plugin.unavailable
notification.delivery_failed
configuration.invalid
```

`notification.delivery_failed` should be specially constrained to prevent the notification system's own failures from creating an infinite notification loop. At minimum, falling back to local-log-only writes should be allowed.

## 8.5 Notification policy

Notification rules and recovery rules must be independent.

Examples:

- A fault appears for the first time: log it.
- A fault persists beyond a threshold: send an alert.
- Automatic recovery succeeds: configurable whether to notify.
- Recovery exhausted: send a high-priority alert.
- Plugin unavailable: notify or not depending on the importance of affected resources.
- The same fault keeps recurring: aggregate and suppress duplicates.
- Fault recovered: send a recovery notification.

## 8.6 Behavior while the network is down

When the network is interrupted:

1. Generate the NotificationEvent.
2. Persist it to the notification queue.
3. Mark it Pending.
4. Do not block the recovery task.
5. A background sender retries with a backoff policy.
6. Processing continues once the network recovers.
7. On success, record Delivered.
8. When the outcome is uncertain, the channel policy decides whether to retry.

Strict end-to-end exactly-once delivery cannot be guaranteed for all external channels. Custos should rely on stable event IDs, best-effort deduplication and observable delivery status.

Suggested states:

```text
Pending
Sending
Delivered
RetryScheduled
DeadLetter
```

`Delivered` only means the channel returned whatever it defines as success; unless the peer provides a confirmation mechanism, Custos cannot claim the end user actually saw the notification.

## 8.7 Security requirements

- Never log tokens, passwords, private keys or full authentication headers.
- Support secure credential references in configuration files.
- Webhook URLs must be validated.
- Restrict plugins' network access permissions.
- Third-party notification plugins must not modify RecoveryRuns directly.
- Notification plugins may only read the event data they need.
- The send queue must not store unnecessary sensitive context.

---

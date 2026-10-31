# ADR-0009: Why Notification is Decoupled

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/notification.md §8

## Background

Notification channels (Webhook/SMTP/herald) depend on network and third-party availability; the recovery engine depends on local resources. Coupling them means: when offline, recovery is stuck on sends; channel failures amplify into system failures; `delivery_failed` re-triggering notifications creates an infinite self-excitation loop.

## Decision

The Core Engine only produces structured `NotificationEvent`s (11 types, custos-model), handed to custos-notify via an EventEmitter callback; the notification engine handles rule matching, enqueueing (SQLite five-state queue), background delivery, exponential backoff, dead-lettering, dedup/aggregation; enqueueing and sending **never block** recovery (§8.1 acceptance); `notification.delivery_failed` goes only to the local log, constructively breaking the self-excitation loop. Persistence goes through the `NotificationQueue` port trait — notify does not reverse-depend on store/engine, the dependency graph stays acyclic.

## Alternatives

- **Recovery engine sends inline**: a slow channel slows recovery; a broken channel breaks recovery; offline events cannot be drained.
- **Shared thread pool direct sends**: no persistence, offline loses events; no retry semantics.

## Pros and Cons

Pros: during outages events all stay queued and drain after recovery (acceptance test offline.rs); channels are pluggable (local-log/webhook/smtp/herald share one Channel trait); sensitive-information governance (scrub) executes in one place.
Cons: an extra queue state machine (Pending/Sending/Delivered/RetryScheduled/DeadLetter); end-to-end is at-least-once, relying on stable event ids + receiver-side deduplication.

## Follow-up Impact

- Webhook/SMTP/herald channels are added per this trait; new channels never touch the core.
- CLI `notify status/retry` reads the queue; recovery and notification acceptance gates are separate.

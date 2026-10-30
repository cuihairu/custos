//! Notification engine (docs/guide/workspace.md: `custos-notify`;
//! docs/guide/notification.md §8).
//!
//! The engine consumes `NotificationEvent`s (custos-model), matches them
//! against the independent `[notification]` rules, and dispatches to
//! configured channels. Delivery failures never block recovery (§8.1):
//! channels return a typed [`ChannelError`] and the queue engine decides
//! retry/backoff/dead-letter (§8.6).
//!
//! P1 scope: local log (base audit output), Webhook and SMTP channels, a
//! persisted queue that keeps working while the network is down.

pub mod channel;
pub mod engine;
pub mod local_log;
pub mod queue;
pub mod smtp;
pub mod webhook;
pub mod webhook_ssrf;

pub use channel::{Channel, ChannelError, ChannelErrorKind};
pub use engine::{DeliverReport, DispatchReport, NotificationEngine};
pub use local_log::{format_event, severity_level, LocalLogChannel};
pub use queue::{NotificationQueue, QueueError, QueuedNotification};
pub use smtp::SmtpChannel;
pub use webhook::{sign_body, WebhookChannel, EVENT_ID_HEADER, SIGNATURE_HEADER};
pub use webhook_ssrf::{check_ip, validate_target, SsrfBlock};

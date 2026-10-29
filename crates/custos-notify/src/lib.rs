//! Event rules, notification queue, retries and channel dispatch
//! (docs/guide/workspace.md: `custos-notify`).
//!
//! P1 scope (docs/guide/notification.md): local log, Webhook and SMTP
//! channels; a persisted queue that keeps working while the network is down;
//! notification failures must never block recovery (§8.1).

/// Placeholder until the notification engine lands in P1.
pub fn not_yet_implemented() -> &'static str {
    "custos-notify: notification engine is planned for P1 (docs/guide/roadmap.md)"
}

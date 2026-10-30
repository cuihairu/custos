//! Store tuning: durability and retention (docs/guide/persistence.md §7.1,
//! §7.4).

use serde::{Deserialize, Serialize};

/// SQLite `synchronous` strategy (§7.1: WAL alone does not imply every
/// write is durable; pick explicitly instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynchronousPolicy {
    /// fastest, may lose recent transactions on power loss
    Off,
    /// good default for WAL; may lose the last transactions on power loss
    Normal,
    /// every commit fsyncs; safest, slowest
    Full,
}

/// Tunables for opening a [`crate::Store`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreConfig {
    pub synchronous: SynchronousPolicy,
    pub retention: RetentionConfig,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            synchronous: SynchronousPolicy::Full,
            retention: RetentionConfig::default(),
        }
    }
}

/// Retention periods (§7.4). `None` keeps the data forever — the safe
/// default. Purging never touches active state regardless of age: open
/// incidents, non-terminal recovery runs and undelivered notifications are
/// exempt by construction in [`crate::Store::purge_retention`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// `audit_events` retention.
    pub audit_retention_ms: Option<u64>,
    /// `probe_results` retention.
    pub probe_detail_retention_ms: Option<u64>,
    /// Terminal `recovery_runs` (with attempts and verifications) and closed
    /// `incidents` retention.
    pub run_retention_ms: Option<u64>,
    /// Delivered `notification_queue` rows and `notification_attempts`
    /// retention.
    pub notification_retention_ms: Option<u64>,
}

impl RetentionConfig {
    /// Cutoff before which rows of a kind are purgeable, if configured.
    fn cutoff(now_unix_ms: u64, retention_ms: Option<u64>) -> Option<i64> {
        retention_ms
            .filter(|ms| *ms > 0)
            .map(|ms| now_unix_ms.saturating_sub(ms) as i64)
    }

    pub fn audit_cutoff(&self, now_unix_ms: u64) -> Option<i64> {
        Self::cutoff(now_unix_ms, self.audit_retention_ms)
    }

    pub fn probe_cutoff(&self, now_unix_ms: u64) -> Option<i64> {
        Self::cutoff(now_unix_ms, self.probe_detail_retention_ms)
    }

    pub fn run_cutoff(&self, now_unix_ms: u64) -> Option<i64> {
        Self::cutoff(now_unix_ms, self.run_retention_ms)
    }

    pub fn notification_cutoff(&self, now_unix_ms: u64) -> Option<i64> {
        Self::cutoff(now_unix_ms, self.notification_retention_ms)
    }
}

/// What one retention purge removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub audit_events: u64,
    pub probe_results: u64,
    pub attempts: u64,
    pub verifications: u64,
    pub runs: u64,
    pub incidents: u64,
    pub notification_attempts: u64,
    pub delivered_notifications: u64,
}

impl PurgeReport {
    pub fn total(&self) -> u64 {
        self.audit_events
            + self.probe_results
            + self.attempts
            + self.verifications
            + self.runs
            + self.incidents
            + self.notification_attempts
            + self.delivered_notifications
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keeps_everything_forever() {
        let retention = RetentionConfig::default();
        assert_eq!(retention.audit_cutoff(1_000_000), None);
        assert_eq!(retention.run_cutoff(1_000_000), None);
    }

    #[test]
    fn cutoff_subtracts_retention_from_now() {
        let retention = RetentionConfig {
            audit_retention_ms: Some(1_000),
            ..RetentionConfig::default()
        };
        assert_eq!(retention.audit_cutoff(10_000), Some(9_000));
        // Saturating: never underflows.
        assert_eq!(retention.audit_cutoff(500), Some(0));
    }
}

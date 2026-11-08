//! Store concern: public row/result types exposed by the store.

use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationQueueRow {
    pub id: u64,
    pub channel: String,
    pub event: NotificationEvent,
    pub state: NotificationState,
    pub attempts: u32,
    pub next_attempt_at_unix_ms: Option<u64>,
    pub last_error: Option<String>,
    pub created_at_unix_ms: u64,
    pub delivered_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NotificationCounts {
    pub pending: u64,
    pub sending: u64,
    pub delivered: u64,
    pub retry_scheduled: u64,
    pub dead_letter: u64,
}

impl NotificationCounts {
    pub fn total(&self) -> u64 {
        self.pending + self.sending + self.delivered + self.retry_scheduled + self.dead_letter
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub runs: u64,
    pub attempts: u64,
    pub verifications: u64,
    pub audit_events: u64,
}

#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: u64,
    pub at_unix_ms: u64,
    pub event_type: String,
    pub severity: String,
    pub resource_id: String,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: String,
    pub resource_id: String,
    pub status: String,
    pub trigger: String,
    pub started_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
pub struct IncidentRow {
    pub id: String,
    pub resource_id: String,
    pub fault_code: String,
    pub summary: String,
    pub opened_at_unix_ms: u64,
    pub closed_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ProbeRow {
    pub resource_id: String,
    pub probe_name: String,
    pub succeeded: bool,
    pub observation: String,
    pub collected_at_unix_ms: u64,
}

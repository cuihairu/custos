//! Probe scheduling and result normalization
//! (docs/guide/workspace.md: `custos-probe`).
//!
//! Builtin probes (docs/guide/probe-survey.md §26, §26.11 order): process,
//! exec, HTTP(S) endpoint, TCP port, DNS resolution, TLS certificate
//! expiry, disk space, memory, CPU load/usage, log keyword, systemd unit,
//! Windows SCM service, Docker container, and the ICMP ping probe. Every
//! probe runs natively (std + rustls + ping + windows-service + libc /
//! windows-sys), never a shell.

pub mod config;
pub mod cpu;
pub mod disk;
pub mod dns;
pub mod docker;
pub mod exec;
pub mod factory;
pub mod http;
pub mod icmp;
pub mod log;
pub mod mem;
pub mod process;
pub mod scheduler;
pub mod sys;
pub mod systemd;
pub mod tcp;
pub mod tls;
pub mod tls_cert;
pub mod windows_service;

use custos_model::ids::ResourceId;
use custos_model::{Evidence, HealthStatus, Resource};

/// One probe verdict, already normalized to a health status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub resource_id: ResourceId,
    pub probe_name: String,
    pub succeeded: bool,
    /// Unreachable means the probe itself could not obtain a valid result
    /// (docs/guide/resources.md §3.2) — not the same as Unhealthy.
    pub status: HealthStatus,
    pub observation: String,
}

impl ProbeReport {
    pub fn into_evidence(self, now_unix_ms: u64) -> Evidence {
        Evidence {
            resource_id: self.resource_id,
            probe_name: self.probe_name,
            succeeded: self.succeeded,
            observation: self.observation,
            collected_at_unix_ms: now_unix_ms,
        }
    }
}

/// A capability that collects facts about one resource. Probes never decide
/// recovery — diagnosis does (docs/guide/probe-diagnosis.md §5.1).
pub trait Probe: Send + Sync {
    fn name(&self) -> &'static str;

    /// Executes the probe against the resource.
    fn probe(&self, resource: &Resource) -> ProbeReport;
}

/// Bookkeeping for periodic probing: one due-timestamp per resource.
#[derive(Debug, Clone, Default)]
pub struct ProbeDueTracker {
    next_due_ms: std::collections::HashMap<ResourceId, u64>,
}

impl ProbeDueTracker {
    /// True once `now` has reached the resource's next due time; the first
    /// call is always due.
    pub fn is_due(&mut self, id: &ResourceId, interval_ms: u64, now_ms: u64) -> bool {
        let due = match self.next_due_ms.get(id) {
            Some(due) => now_ms >= *due,
            None => true,
        };
        if due {
            self.next_due_ms.insert(id.clone(), now_ms + interval_ms);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_probe_is_due_and_schedules_next() {
        let mut tracker = ProbeDueTracker::default();
        let id = ResourceId::new("svc");
        assert!(tracker.is_due(&id, 1000, 0));
        assert!(!tracker.is_due(&id, 1000, 999));
        assert!(tracker.is_due(&id, 1000, 1000));
    }
}

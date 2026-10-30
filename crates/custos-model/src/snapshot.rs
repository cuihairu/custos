//! Per-resource health bookkeeping (docs/guide/resources.md §3.2).

use crate::diagnosis::FaultCode;
use crate::health::HealthStatus;
use crate::ids::{RecoveryRunId, ResourceId};
use serde::{Deserialize, Serialize};

/// Mutable health state tracked alongside a resource definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceHealthSnapshot {
    pub resource_id: ResourceId,
    pub status: HealthStatus,
    pub consecutive_successes: u32,
    pub consecutive_failures: u32,
    pub last_probe_unix_ms: Option<u64>,
    pub active_fault: Option<FaultCode>,
    pub active_run: Option<RecoveryRunId>,
    pub updated_at_unix_ms: u64,
}

impl ResourceHealthSnapshot {
    pub fn new(resource_id: ResourceId, now_unix_ms: u64) -> Self {
        Self {
            resource_id,
            status: HealthStatus::Unknown,
            consecutive_successes: 0,
            consecutive_failures: 0,
            last_probe_unix_ms: None,
            active_fault: None,
            active_run: None,
            updated_at_unix_ms: now_unix_ms,
        }
    }

    /// Applies one probe verdict with the resource's thresholds and updates
    /// the status, including jitter suppression (docs/guide/probe-diagnosis.md
    /// §5.3): a single timeout must not flip the state.
    pub fn record_probe(
        &mut self,
        succeeded: bool,
        failure_threshold: u32,
        success_threshold: u32,
        now_unix_ms: u64,
    ) {
        self.last_probe_unix_ms = Some(now_unix_ms);
        self.updated_at_unix_ms = now_unix_ms;
        if succeeded {
            self.consecutive_successes += 1;
            self.consecutive_failures = 0;
            if self.consecutive_successes >= success_threshold {
                self.status = HealthStatus::Healthy;
                self.active_fault = None;
            } else if self.status == HealthStatus::Unknown || self.status == HealthStatus::Unhealthy
            {
                // still below recovery confirmation window
            }
        } else {
            self.consecutive_failures += 1;
            self.consecutive_successes = 0;
            if self.consecutive_failures >= failure_threshold {
                self.status = HealthStatus::Unhealthy;
                if self.active_fault.is_none() {
                    self.active_fault = Some(FaultCode::ProbeTimeout);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(now: u64) -> ResourceHealthSnapshot {
        ResourceHealthSnapshot::new(ResourceId::new("svc"), now)
    }

    #[test]
    fn single_failure_does_not_flip_state() {
        let mut s = snapshot(0);
        s.record_probe(false, 3, 2, 10);
        assert_eq!(s.status, HealthStatus::Unknown);
        assert_eq!(s.consecutive_failures, 1);
    }

    #[test]
    fn failure_threshold_enters_unhealthy() {
        let mut s = snapshot(0);
        for i in 1..=3 {
            s.record_probe(false, 3, 2, i);
        }
        assert_eq!(s.status, HealthStatus::Unhealthy);
        assert_eq!(s.consecutive_failures, 3);
        assert_eq!(s.active_fault, Some(FaultCode::ProbeTimeout));
    }

    #[test]
    fn success_threshold_restores_healthy() {
        let mut s = snapshot(0);
        for i in 1..=3 {
            s.record_probe(false, 3, 2, i);
        }
        for i in 4..=5 {
            s.record_probe(true, 3, 2, i);
        }
        assert_eq!(s.status, HealthStatus::Healthy);
        assert_eq!(s.active_fault, None);
        assert_eq!(s.consecutive_successes, 2);
    }
}

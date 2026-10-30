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
    ///
    /// The probe's normalized status matters (docs/guide/resources.md §3.2):
    /// `Unreachable` — the probe itself could not obtain a valid result —
    /// never confirms a fault, so it never crosses into `Unhealthy`. Sustained
    /// unreachability marks the resource `Unreachable` (unprobeable), which
    /// does not trigger recovery.
    pub fn record_probe(
        &mut self,
        succeeded: bool,
        probe_status: HealthStatus,
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
                match probe_status {
                    // "Cannot observe" never overwrites a confirmed fault: an
                    // unhealthy resource whose probe starts timing out keeps
                    // its fault visible (and its recovery runnable).
                    HealthStatus::Unreachable
                        if matches!(
                            self.status,
                            HealthStatus::Unhealthy
                                | HealthStatus::Recovering
                                | HealthStatus::Suppressed
                        ) => {}
                    _ => self.status = probe_status,
                }
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
        s.record_probe(false, HealthStatus::Unhealthy, 3, 2, 10);
        assert_eq!(s.status, HealthStatus::Unknown);
        assert_eq!(s.consecutive_failures, 1);
    }

    #[test]
    fn failure_threshold_enters_unhealthy() {
        let mut s = snapshot(0);
        for i in 1..=3 {
            s.record_probe(false, HealthStatus::Unhealthy, 3, 2, i);
        }
        assert_eq!(s.status, HealthStatus::Unhealthy);
        assert_eq!(s.consecutive_failures, 3);
        assert_eq!(s.active_fault, Some(FaultCode::ProbeTimeout));
    }

    #[test]
    fn success_threshold_restores_healthy() {
        let mut s = snapshot(0);
        for i in 1..=3 {
            s.record_probe(false, HealthStatus::Unhealthy, 3, 2, i);
        }
        for i in 4..=5 {
            s.record_probe(true, HealthStatus::Healthy, 3, 2, i);
        }
        assert_eq!(s.status, HealthStatus::Healthy);
        assert_eq!(s.active_fault, None);
        assert_eq!(s.consecutive_successes, 2);
    }

    #[test]
    fn sustained_unreachable_never_confirms_a_fault() {
        // §3.2: the probe could not obtain a valid result, so the resource is
        // marked unprobeable, never Unhealthy.
        let mut s = snapshot(0);
        for i in 1..=3 {
            s.record_probe(false, HealthStatus::Unreachable, 3, 2, i);
        }
        assert_eq!(s.status, HealthStatus::Unreachable);
        assert_eq!(s.consecutive_failures, 3);
        // A later definite verdict still works: unreachable streak then a
        // healthy probe restores Healthy per the success threshold.
        for i in 4..=5 {
            s.record_probe(true, HealthStatus::Healthy, 3, 2, i);
        }
        assert_eq!(s.status, HealthStatus::Healthy);
    }

    #[test]
    fn unreachable_after_unhealthy_keeps_the_fault_visible() {
        let mut s = snapshot(0);
        for i in 1..=3 {
            s.record_probe(false, HealthStatus::Unhealthy, 3, 2, i);
        }
        assert_eq!(s.status, HealthStatus::Unhealthy);
        // The probe then times out: the earlier fault is not overwritten by
        // a "could not obtain a result" verdict.
        s.record_probe(false, HealthStatus::Unreachable, 3, 2, 4);
        assert_eq!(s.status, HealthStatus::Unhealthy);
    }
}

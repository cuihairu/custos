//! Recovery budgets (docs/guide/recovery-engine.md §6.5). Every mandatory
//! limit that is reached must terminate the task or escalate it to a human.

use custos_model::ids::ResourceId;
use std::collections::HashMap;

/// Mandatory limits; defaults are conservative P0 values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryBudget {
    /// Maximum recovery attempts per resource (across plans, per incident).
    pub max_attempts_per_resource: u32,
    /// Wall-clock ceiling for a single RecoveryRun.
    pub max_run_duration_ms: u64,
    /// Sliding window for the restart-rate limit.
    pub restart_window_ms: u64,
    /// Maximum restarts per resource inside the window.
    pub max_restarts_per_window: u32,
    /// Maximum high-risk action executions per RecoveryRun.
    pub max_high_risk_actions: u32,
    /// Maximum simultaneously active RecoveryRuns (global).
    pub max_concurrent_runs: usize,
}

impl Default for RecoveryBudget {
    fn default() -> Self {
        Self {
            max_attempts_per_resource: 6,
            max_run_duration_ms: 10 * 60 * 1000,
            restart_window_ms: 10 * 60 * 1000,
            max_restarts_per_window: 5,
            max_high_risk_actions: 1,
            max_concurrent_runs: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetExceeded {
    #[error("attempt limit for `{resource_id}` reached ({limit})")]
    AttemptsPerResource { resource_id: ResourceId, limit: u32 },
    #[error("run duration limit reached ({limit_ms} ms)")]
    RunDuration { limit_ms: u64 },
    #[error("restart rate limit for `{resource_id}` reached ({limit} per window)")]
    RestartRate { resource_id: ResourceId, limit: u32 },
    #[error("high-risk action limit reached ({limit})")]
    HighRiskActions { limit: u32 },
    #[error("concurrent run limit reached ({limit})")]
    ConcurrentRuns { limit: usize },
}

#[derive(Debug, Default)]
pub struct BudgetState {
    attempts: HashMap<ResourceId, u32>,
    restarts: HashMap<ResourceId, Vec<u64>>,
    high_risk_used: u32,
}

impl BudgetState {
    pub fn check_start(
        &self,
        resource_id: &ResourceId,
        budget: &RecoveryBudget,
        active_runs: usize,
    ) -> Result<(), BudgetExceeded> {
        if active_runs >= budget.max_concurrent_runs {
            return Err(BudgetExceeded::ConcurrentRuns {
                limit: budget.max_concurrent_runs,
            });
        }
        if let Some(&used) = self.attempts.get(resource_id) {
            if used >= budget.max_attempts_per_resource {
                return Err(BudgetExceeded::AttemptsPerResource {
                    resource_id: resource_id.clone(),
                    limit: budget.max_attempts_per_resource,
                });
            }
        }
        Ok(())
    }

    /// Per-run duration check; call before each attempt.
    pub fn check_run_duration(
        &self,
        started_at_unix_ms: u64,
        now_unix_ms: u64,
        budget: &RecoveryBudget,
    ) -> Result<(), BudgetExceeded> {
        if now_unix_ms.saturating_sub(started_at_unix_ms) >= budget.max_run_duration_ms {
            return Err(BudgetExceeded::RunDuration {
                limit_ms: budget.max_run_duration_ms,
            });
        }
        Ok(())
    }

    /// Rate-limits restart-like actions inside the sliding window.
    pub fn check_restart_rate(
        &self,
        resource_id: &ResourceId,
        budget: &RecoveryBudget,
        now_unix_ms: u64,
    ) -> Result<(), BudgetExceeded> {
        let recent = self
            .restarts
            .get(resource_id)
            .map(|ts| {
                ts.iter()
                    .filter(|t| now_unix_ms.saturating_sub(**t) < budget.restart_window_ms)
                    .count()
            })
            .unwrap_or(0);
        if recent as u32 >= budget.max_restarts_per_window {
            return Err(BudgetExceeded::RestartRate {
                resource_id: resource_id.clone(),
                limit: budget.max_restarts_per_window,
            });
        }
        Ok(())
    }

    pub fn check_high_risk(&self, budget: &RecoveryBudget) -> Result<(), BudgetExceeded> {
        if self.high_risk_used >= budget.max_high_risk_actions {
            return Err(BudgetExceeded::HighRiskActions {
                limit: budget.max_high_risk_actions,
            });
        }
        Ok(())
    }

    /// Records one executed attempt; every recorder implies a restart-like
    /// event for the sliding-window limiter.
    pub fn record_attempt(&mut self, resource_id: &ResourceId, now_unix_ms: u64) {
        *self.attempts.entry(resource_id.clone()).or_insert(0) += 1;
        self.restarts
            .entry(resource_id.clone())
            .or_default()
            .push(now_unix_ms);
    }

    pub fn record_high_risk(&mut self) {
        self.high_risk_used += 1;
    }

    pub fn attempts_for(&self, resource_id: &ResourceId) -> u32 {
        self.attempts.get(resource_id).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempts_per_resource_limit_trips() {
        let budget = RecoveryBudget {
            max_attempts_per_resource: 2,
            ..RecoveryBudget::default()
        };
        let mut state = BudgetState::default();
        let id = ResourceId::new("svc");
        state.record_attempt(&id, 0);
        state.record_attempt(&id, 1);
        assert!(matches!(
            state.check_start(&id, &budget, 0),
            Err(BudgetExceeded::AttemptsPerResource { .. })
        ));
    }

    #[test]
    fn restart_rate_window_slides() {
        let budget = RecoveryBudget {
            restart_window_ms: 1_000,
            max_restarts_per_window: 2,
            ..RecoveryBudget::default()
        };
        let mut state = BudgetState::default();
        let id = ResourceId::new("svc");
        state.record_attempt(&id, 0);
        state.record_attempt(&id, 500);
        assert!(state.check_restart_rate(&id, &budget, 600).is_err());
        // After the window slides past both, the limit frees up again.
        assert!(state.check_restart_rate(&id, &budget, 1_600).is_ok());
    }

    #[test]
    fn run_duration_limit_trips() {
        let state = BudgetState::default();
        let budget = RecoveryBudget {
            max_run_duration_ms: 1_000,
            ..RecoveryBudget::default()
        };
        assert!(state.check_run_duration(0, 999, &budget).is_ok());
        assert!(state.check_run_duration(0, 1_000, &budget).is_err());
    }

    #[test]
    fn concurrent_runs_limit_trips() {
        let state = BudgetState::default();
        let budget = RecoveryBudget {
            max_concurrent_runs: 1,
            ..RecoveryBudget::default()
        };
        assert!(state.check_start(&ResourceId::new("a"), &budget, 0).is_ok());
        assert!(state
            .check_start(&ResourceId::new("b"), &budget, 1)
            .is_err());
    }
}

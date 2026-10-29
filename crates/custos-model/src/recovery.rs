//! Recovery execution model and the RecoveryRun state machine
//! (docs/guide/recovery-engine.md §6.1–§6.6).

use crate::diagnosis::Diagnosis;
use crate::ids::{ActionId, IncidentId, RecoveryRunId, ResourceId};
use serde::{Deserialize, Serialize};

/// Result of one action execution. There are more than success and failure:
/// an action whose outcome is unknown must never be blindly repeated (§6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    OutcomeUnknown,
}

/// States of a RecoveryRun (docs/guide/recovery-engine.md §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRunStatus {
    Observing,
    ProblemDetected,
    Diagnosing,
    Planning,
    ResolvingDependencies,
    Executing,
    Verifying,
    Recovered,
    Escalated,
    Suppressed,
    Waiting,
    Cancelled,
    Interrupted,
    OutcomeUnknown,
}

impl RecoveryRunStatus {
    /// Legal transitions from the state diagram in §6.1, plus the additional
    /// states it requires (`Waiting`, `Cancelled`, `Interrupted`,
    /// `OutcomeUnknown`). Terminal states have no outgoing edges; state must
    /// never be modified arbitrarily in business code.
    pub fn allowed_transitions(self) -> &'static [RecoveryRunStatus] {
        use RecoveryRunStatus::*;
        match self {
            Observing => &[ProblemDetected, Cancelled],
            ProblemDetected => &[Diagnosing, Suppressed, Cancelled],
            Diagnosing => &[Planning, Suppressed, Escalated, Cancelled],
            Planning => &[ResolvingDependencies, Suppressed, Escalated, Cancelled],
            ResolvingDependencies => &[Planning, Executing, Escalated, Cancelled],
            Executing => &[Verifying, Cancelled, Interrupted, OutcomeUnknown],
            Verifying => &[Recovered, Diagnosing, OutcomeUnknown, Cancelled],
            // Terminal states.
            Recovered | Escalated | Suppressed | Cancelled | Interrupted | OutcomeUnknown => &[],
            // `Waiting` resumes into planning once its wait condition is
            // re-evaluated; it may also be cancelled outright.
            Waiting => &[Planning, Cancelled],
        }
    }

    pub fn is_terminal(self) -> bool {
        self.allowed_transitions().is_empty()
    }

    /// Checks one transition and returns [`TransitionError`] for illegal ones.
    pub fn transition_to(self, next: RecoveryRunStatus) -> Result<Self, TransitionError> {
        if self.allowed_transitions().contains(&next) {
            Ok(next)
        } else {
            Err(TransitionError {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal state transition: {from:?} -> {to:?}")]
pub struct TransitionError {
    pub from: RecoveryRunStatus,
    pub to: RecoveryRunStatus,
}

/// One recorded attempt to execute a recovery action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionAttempt {
    pub action_id: ActionId,
    pub outcome: ActionOutcome,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
}

/// One candidate recovery action (docs/guide/recovery-engine.md §6.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryAction {
    pub action_id: ActionId,
    /// Applicable fault types; the engine selects by diagnosis, not blindly.
    pub applicable_faults: Vec<crate::diagnosis::FaultCode>,
    pub risk_level: RiskLevel,
    pub max_attempts: u32,
}

/// Risk classification (docs/guide/security.md §12.2). High-risk actions
/// require explicit configuration opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// One complete failure-handling process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryRun {
    pub id: RecoveryRunId,
    pub resource_id: ResourceId,
    pub incident_id: Option<IncidentId>,
    pub status: RecoveryRunStatus,
    pub trigger: String,
    pub diagnosis: Option<Diagnosis>,
    pub action_attempts: Vec<ActionAttempt>,
    pub started_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

impl RecoveryRun {
    /// Moves the run to `next` through the checked transition.
    pub fn transition_to(&mut self, next: RecoveryRunStatus) -> Result<(), TransitionError> {
        self.status = self.status.transition_to(next)?;
        self.updated_at_unix_ms += 1; // real clock is injected by the engine
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_transitions_are_legal() {
        let mut run = RecoveryRun {
            id: RecoveryRunId::new("run-1"),
            resource_id: ResourceId::new("proxy.egress"),
            incident_id: Some(IncidentId::new("inc-1")),
            status: RecoveryRunStatus::Observing,
            trigger: "failure threshold reached".into(),
            diagnosis: None,
            action_attempts: vec![],
            started_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        // Observing -> ProblemDetected -> Diagnosing -> Planning ->
        // ResolvingDependencies -> Executing -> Verifying -> Recovered
        for next in [
            RecoveryRunStatus::ProblemDetected,
            RecoveryRunStatus::Diagnosing,
            RecoveryRunStatus::Planning,
            RecoveryRunStatus::ResolvingDependencies,
            RecoveryRunStatus::Executing,
            RecoveryRunStatus::Verifying,
            RecoveryRunStatus::Recovered,
        ] {
            run.transition_to(next)
                .unwrap_or_else(|e| panic!("{next:?} should be legal: {e}"));
        }
        assert!(run.status.is_terminal());
    }

    #[test]
    fn illegal_transition_is_rejected() {
        // Executing may not jump straight to Recovered: verification is
        // mandatory (docs/guide/recovery-engine.md §6.1).
        let err = RecoveryRunStatus::Executing
            .transition_to(RecoveryRunStatus::Recovered)
            .unwrap_err();
        assert_eq!(err.from, RecoveryRunStatus::Executing);
        assert_eq!(err.to, RecoveryRunStatus::Recovered);
    }

    #[test]
    fn terminal_states_have_no_outgoing_edges() {
        for state in [
            RecoveryRunStatus::Recovered,
            RecoveryRunStatus::Escalated,
            RecoveryRunStatus::Suppressed,
            RecoveryRunStatus::Cancelled,
            RecoveryRunStatus::Interrupted,
            RecoveryRunStatus::OutcomeUnknown,
        ] {
            assert!(
                state.allowed_transitions().is_empty(),
                "{state:?} must be terminal"
            );
        }
    }

    #[test]
    fn verifying_can_reenter_diagnosing_when_fault_remains() {
        RecoveryRunStatus::Verifying
            .transition_to(RecoveryRunStatus::Diagnosing)
            .unwrap();
    }

    #[test]
    fn action_outcome_serializes_all_variants() {
        for outcome in [
            ActionOutcome::Succeeded,
            ActionOutcome::Failed,
            ActionOutcome::TimedOut,
            ActionOutcome::Cancelled,
            ActionOutcome::OutcomeUnknown,
        ] {
            let json = serde_json::to_string(&outcome).unwrap();
            let back: ActionOutcome = serde_json::from_str(&json).unwrap();
            assert_eq!(back, outcome);
        }
    }
}

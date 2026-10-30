//! Events the engine emits for auditing and notification. The Core Engine
//! produces structured events only (docs/guide/notification.md §8.1); the
//! notification subsystem subscribes through the store/sink.

use custos_model::ids::{RecoveryRunId, ResourceId};
use custos_model::{ActionOutcome, RecoveryRun};

/// Severity carried on notification-worthy events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// Structured engine event: every state transition and action attempt must
/// be auditable (docs/guide/agent-protocol.md §11).
#[derive(Debug, Clone)]
pub enum EngineEvent {
    RecoveryStarted {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        at_unix_ms: u64,
    },
    ActionStarted {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        plan_id: String,
        attempt_no: u32,
        at_unix_ms: u64,
    },
    ActionFinished {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        plan_id: String,
        attempt_no: u32,
        outcome: ActionOutcome,
        at_unix_ms: u64,
    },
    VerificationRecorded {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        passed: bool,
        at_unix_ms: u64,
    },
    RecoverySucceeded {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        at_unix_ms: u64,
    },
    RecoveryExhausted {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        at_unix_ms: u64,
    },
    /// An attempt was found with unknown outcome (crash recovery) — it must
    /// never be blindly repeated (docs/guide/recovery-engine.md §6.6).
    OutcomeUnknown {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        at_unix_ms: u64,
    },
    /// Dependency recovery failed; upstream actions were not executed
    /// (docs/guide/agent-brief.md rule 5).
    DependencyUnsatisfied {
        run_id: RecoveryRunId,
        resource_id: ResourceId,
        dependency_id: ResourceId,
        at_unix_ms: u64,
    },
}

impl EngineEvent {
    /// The event type key used in logs, audit rows and notifications.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::RecoveryStarted { .. } => "recovery.started",
            Self::ActionStarted { .. } => "recovery.action_started",
            Self::ActionFinished { .. } => "recovery.action_finished",
            Self::VerificationRecorded { .. } => "recovery.verification",
            Self::RecoverySucceeded { .. } => "recovery.succeeded",
            Self::RecoveryExhausted { .. } => "recovery.exhausted",
            Self::OutcomeUnknown { .. } => "recovery.outcome_unknown",
            Self::DependencyUnsatisfied { .. } => "dependency.unsatisfied",
        }
    }

    pub fn resource_id(&self) -> &ResourceId {
        match self {
            Self::RecoveryStarted { resource_id, .. }
            | Self::ActionStarted { resource_id, .. }
            | Self::ActionFinished { resource_id, .. }
            | Self::VerificationRecorded { resource_id, .. }
            | Self::RecoverySucceeded { resource_id, .. }
            | Self::RecoveryExhausted { resource_id, .. }
            | Self::OutcomeUnknown { resource_id, .. }
            | Self::DependencyUnsatisfied { resource_id, .. } => resource_id,
        }
    }

    pub fn at_unix_ms(&self) -> u64 {
        match self {
            Self::RecoveryStarted { at_unix_ms, .. }
            | Self::ActionStarted { at_unix_ms, .. }
            | Self::ActionFinished { at_unix_ms, .. }
            | Self::VerificationRecorded { at_unix_ms, .. }
            | Self::RecoverySucceeded { at_unix_ms, .. }
            | Self::RecoveryExhausted { at_unix_ms, .. }
            | Self::OutcomeUnknown { at_unix_ms, .. }
            | Self::DependencyUnsatisfied { at_unix_ms, .. } => *at_unix_ms,
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            Self::RecoveryStarted { .. }
            | Self::ActionStarted { .. }
            | Self::VerificationRecorded { .. }
            | Self::RecoverySucceeded { .. } => Severity::Info,
            Self::ActionFinished { .. } | Self::OutcomeUnknown { .. } => Severity::Warning,
            Self::RecoveryExhausted { .. } | Self::DependencyUnsatisfied { .. } => {
                Severity::Critical
            }
        }
    }
}

/// Where the engine persists runs and emits events. Implemented by
/// `custos-store` (SQLite) and by an in-memory recorder in tests.
pub trait RunStore: Send + Sync {
    /// Persists (upserts) the run and its status.
    fn save_run(&self, run: &RecoveryRun);

    /// Persists one attempt. Called twice per attempt: once with
    /// `outcome: None` (state `Executing`, before the external action runs —
    /// docs/guide/persistence.md §7.3 step 5) and once with the result.
    fn save_attempt(&self, run_id: &RecoveryRunId, attempt: &custos_model::ActionAttempt);

    fn save_verification(&self, result: &custos_model::VerificationResult);

    /// Emits an audit/notification event.
    fn emit(&self, event: EngineEvent);

    /// Runs that never reached a terminal state — the crash-recovery input.
    fn unfinished_runs(&self) -> Vec<RecoveryRun>;
}

/// In-memory store for tests: full run bookkeeping plus an event log.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: std::sync::Mutex<MemoryInner>,
}

#[derive(Debug, Default)]
struct MemoryInner {
    runs: Vec<RecoveryRun>,
    verifications: Vec<custos_model::VerificationResult>,
    events: Vec<EngineEvent>,
}

impl MemoryStore {
    pub fn events(&self) -> Vec<EngineEvent> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .events
            .clone()
    }

    pub fn event_types(&self) -> Vec<&'static str> {
        self.events().iter().map(|e| e.event_type()).collect()
    }

    pub fn run(&self, id: &RecoveryRunId) -> Option<RecoveryRun> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .runs
            .iter()
            .find(|r| &r.id == id)
            .cloned()
    }
}

impl RunStore for MemoryStore {
    fn save_run(&self, run: &RecoveryRun) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match inner.runs.iter_mut().find(|r| r.id == run.id) {
            Some(existing) => *existing = run.clone(),
            None => inner.runs.push(run.clone()),
        }
    }

    fn save_attempt(&self, run_id: &RecoveryRunId, attempt: &custos_model::ActionAttempt) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(run) = inner.runs.iter_mut().find(|r| r.id == *run_id) {
            match run
                .action_attempts
                .iter_mut()
                .find(|a| a.action_id == attempt.action_id)
            {
                Some(existing) => *existing = attempt.clone(),
                None => run.action_attempts.push(attempt.clone()),
            }
        }
    }

    fn save_verification(&self, result: &custos_model::VerificationResult) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .verifications
            .push(result.clone());
    }

    fn emit(&self, event: EngineEvent) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .events
            .push(event);
    }

    fn unfinished_runs(&self) -> Vec<RecoveryRun> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .runs
            .iter()
            .filter(|r| !r.status.is_terminal())
            .cloned()
            .collect()
    }
}

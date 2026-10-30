//! Action execution. The engine owns execution; plugins provide actions but
//! never bypass the core (docs/guide/agent-brief.md rule 4).

use custos_model::{ActionOutcome, ActionSpec};
use std::time::Duration;

/// Executes one recovery action with a hard timeout. Implementations must
/// never swallow a crash: if the result cannot be determined, return
/// [`ActionOutcome::OutcomeUnknown`] (docs/guide/recovery-engine.md §6.6).
pub trait ActionExecutor: Send + Sync {
    fn execute(&self, action: &ActionSpec, timeout: Duration) -> ActionOutcome;
}

/// Closures are executors too — handy for tests and CLI wiring.
impl<F> ActionExecutor for F
where
    F: Fn(&ActionSpec, Duration) -> ActionOutcome + Send + Sync,
{
    fn execute(&self, action: &ActionSpec, timeout: Duration) -> ActionOutcome {
        self(action, timeout)
    }
}

/// Builtin exec executor: runs the parameterized argv under the same
/// controlled-execution rules as the Exec Probe (user ruling 2026-10-29:
/// builtin base adapters).
pub struct BuiltinExecExecutor;

impl ActionExecutor for BuiltinExecExecutor {
    fn execute(&self, action: &ActionSpec, timeout: Duration) -> ActionOutcome {
        let argv: Vec<String> = std::iter::once(action.program.clone())
            .chain(action.args.iter().cloned())
            .collect();
        match custos_probe::exec::run_controlled(&argv, timeout, 64 * 1024) {
            custos_probe::exec::ExecResult::Succeeded { .. } => ActionOutcome::Succeeded,
            custos_probe::exec::ExecResult::Failed { code, .. } => {
                let _ = code;
                ActionOutcome::Failed
            }
            custos_probe::exec::ExecResult::TimedOut => ActionOutcome::TimedOut,
            custos_probe::exec::ExecResult::StartFailed(_) => ActionOutcome::Failed,
        }
    }
}

/// Test double with scripted outcomes.
#[derive(Default)]
pub struct ScriptedExecutor {
    steps: std::sync::Mutex<std::collections::VecDeque<ActionOutcome>>,
}

impl ScriptedExecutor {
    pub fn new(steps: Vec<ActionOutcome>) -> Self {
        Self {
            steps: std::sync::Mutex::new(steps.into()),
        }
    }

    /// Steps not yet consumed — tests assert actions never ran.
    pub fn remaining(&self) -> usize {
        self.steps.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl ActionExecutor for ScriptedExecutor {
    fn execute(&self, _action: &ActionSpec, _timeout: Duration) -> ActionOutcome {
        self.steps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or(ActionOutcome::Failed)
    }
}

//! Action execution. The engine owns execution; plugins provide actions but
//! never bypass the core (docs/guide/agent-brief.md rule 4).

use custos_model::{ActionOutcome, ActionSpec};
use std::sync::Arc;
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

/// Extra time granted to an executor past its own `timeout` deadline before
/// the watchdog gives up on it (C20). Covers legitimate overshoot — killing
/// a process tree, joining pipe readers — without trusting that every
/// executor actually honors the deadline it was handed.
pub const EXECUTOR_WATCHDOG_GRACE_MS: u64 = 5_000;

/// Runs one action under a hard watchdog (C20). Recovery actions execute on
/// the tick thread, so an executor that hangs — a plugin ignoring its
/// deadline, or the builtin executor blocked on a daemon that escaped the
/// process group and holds a pipe open — would stall every future tick.
/// The executor runs on a worker thread; past `timeout +
/// EXECUTOR_WATCHDOG_GRACE_MS` the result is unknowable (§6.6), the attempt
/// records [`ActionOutcome::OutcomeUnknown`] and the tick moves on. The
/// worker stays parked on the executor until it finally returns; a result
/// arriving that late is logged and discarded.
pub fn execute_with_watchdog(
    executor: Arc<dyn ActionExecutor>,
    action: &ActionSpec,
    timeout: Duration,
) -> ActionOutcome {
    execute_with_deadline(
        executor,
        action,
        timeout,
        timeout + Duration::from_millis(EXECUTOR_WATCHDOG_GRACE_MS),
    )
}

fn execute_with_deadline(
    executor: Arc<dyn ActionExecutor>,
    action: &ActionSpec,
    timeout: Duration,
    deadline: Duration,
) -> ActionOutcome {
    let owned = action.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = executor.execute(&owned, timeout);
        if tx.send(outcome).is_err() {
            tracing::warn!(
                target: "custos::engine",
                "action finished after the watchdog gave up; result discarded"
            );
        }
    });
    match rx.recv_timeout(deadline) {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::warn!(
                target: "custos::engine",
                "action exceeded its watchdog deadline; outcome unknown (§6.6)"
            );
            ActionOutcome::OutcomeUnknown
        }
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
            // A start failure is permanent for the plan (§6.6): the action
            // never ran, so `Failed` would let the engine retry the same
            // doomed start until the ladder ran dry (C21).
            custos_probe::exec::ExecResult::StartFailed(_) => ActionOutcome::NotStarted,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> ActionSpec {
        ActionSpec {
            program: "true".to_owned(),
            args: vec![],
        }
    }

    #[test]
    fn watchdog_passes_through_a_timely_outcome() {
        let executor: Arc<dyn ActionExecutor> = Arc::new(|_: &ActionSpec, _: Duration| {
            std::thread::sleep(Duration::from_millis(20));
            ActionOutcome::Succeeded
        });
        let outcome = execute_with_deadline(
            executor,
            &action(),
            Duration::from_secs(1),
            Duration::from_millis(2_000),
        );
        assert_eq!(outcome, ActionOutcome::Succeeded);
    }

    #[test]
    fn watchdog_declares_unknown_when_the_executor_never_returns() {
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = std::sync::Mutex::new(gate_rx);
        let executor: Arc<dyn ActionExecutor> = Arc::new(move |_: &ActionSpec, _: Duration| {
            let _ = gate_rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
            ActionOutcome::Succeeded
        });
        let started = std::time::Instant::now();
        let outcome = execute_with_deadline(
            executor,
            &action(),
            Duration::from_millis(1),
            Duration::from_millis(50),
        );
        assert_eq!(outcome, ActionOutcome::OutcomeUnknown);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "watchdog waited far past its deadline"
        );
        // Release the parked worker; the late result must be dropped, not
        // delivered or panicked on.
        drop(gate_tx);
    }
}

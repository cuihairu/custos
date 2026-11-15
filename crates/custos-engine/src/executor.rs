//! Action execution. The engine owns execution; plugins provide actions but
//! never bypass the core (docs/guide/agent-brief.md rule 4).

use custos_model::{ActionOutcome, ActionSpec};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
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

/// Number of watchdog workers still parked on an executor past their
/// deadline (2026-11-11 audit P0-2). Threads cannot be force-killed in
/// Rust, so a hung executor leaks one worker (and the executor `Arc`) per
/// attempt; the counter makes the leak observable — log scrapes and tests
/// assert it stays at zero for executors that honor their deadline. The
/// builtin executor always returns since the P0-1 drain deadline; plugin
/// executors that can hang are visible here until they are moved to
/// out-of-process isolation.
static LEAKED_WATCHDOG_WORKERS: AtomicUsize = AtomicUsize::new(0);

/// Watchdog workers currently parked past their deadline. Zero means every
/// executor returned within `timeout + EXECUTOR_WATCHDOG_GRACE_MS`.
pub fn leaked_watchdog_workers() -> usize {
    LEAKED_WATCHDOG_WORKERS.load(Ordering::Relaxed)
}

/// Runs one action under a hard watchdog (C20). Recovery actions execute on
/// the tick thread, so an executor that hangs — a plugin ignoring its
/// deadline, or the builtin executor blocked on a daemon that escaped the
/// process group and holds a pipe open — would stall every future tick.
/// The executor runs on a worker thread; past `timeout +
/// EXECUTOR_WATCHDOG_GRACE_MS` the result is unknowable (§6.6), the attempt
/// records [`ActionOutcome::OutcomeUnknown`] and the tick moves on. The
/// worker stays parked on the executor until it finally returns (counted in
/// [`leaked_watchdog_workers`]); a result arriving that late is logged and
/// discarded. Accounting goes through a per-execution [`WatchdogTicket`]:
/// a late result can never leave the leak count inflated, and a worker
/// that died mid-execute (panic) is dead, not parked, so it is not counted.
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

/// Per-execution watchdog accounting. The global leak counter must only
/// hold workers that are actually parked, so the +1/−1 decisions cannot be
/// made from the send outcome alone: a late send that wins the race against
/// the receiver being dropped succeeds and would otherwise leave a booked
/// count standing forever. The ticket reconciles both sides — the main
/// thread books a leak only if the worker has not finished yet, and the
/// worker clears the booking when it eventually returns (2026-11-12 Codex
/// audit: the timeout/late-result race previously inflated the count).
#[derive(Default)]
struct WatchdogTicket {
    /// The worker finished executing (outcome sent or receiver gone).
    worker_done: bool,
    /// The main thread booked one leaked-worker count for this execution.
    booked: bool,
}

fn execute_with_deadline(
    executor: Arc<dyn ActionExecutor>,
    action: &ActionSpec,
    timeout: Duration,
    deadline: Duration,
) -> ActionOutcome {
    let owned = action.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let ticket = Arc::new(Mutex::new(WatchdogTicket::default()));
    let worker_ticket = Arc::clone(&ticket);
    std::thread::spawn(move || {
        let outcome = executor.execute(&owned, timeout);
        let _ = tx.send(outcome);
        let mut state = worker_ticket.lock().unwrap_or_else(|e| e.into_inner());
        state.worker_done = true;
        if state.booked {
            state.booked = false;
            LEAKED_WATCHDOG_WORKERS.fetch_sub(1, Ordering::Relaxed);
            let leaked = LEAKED_WATCHDOG_WORKERS.load(Ordering::Relaxed);
            drop(state);
            tracing::warn!(
                target: "custos::engine",
                leaked,
                "action finished after the watchdog gave up; result discarded"
            );
        }
    });
    match rx.recv_timeout(deadline) {
        Ok(outcome) => outcome,
        Err(RecvTimeoutError::Timeout) => {
            let mut state = ticket.lock().unwrap_or_else(|e| e.into_inner());
            // A worker that already finished is not parked: recv_timeout
            // may report Timeout even though the send landed first, and
            // booking that worker would inflate the count forever.
            if !state.worker_done {
                state.booked = true;
                LEAKED_WATCHDOG_WORKERS.fetch_add(1, Ordering::Relaxed);
                drop(state);
                tracing::warn!(
                    target: "custos::engine",
                    "action exceeded its watchdog deadline; outcome unknown (§6.6)"
                );
            }
            ActionOutcome::OutcomeUnknown
        }
        Err(RecvTimeoutError::Disconnected) => {
            // tx dropped without a send: the worker panicked mid-execute.
            // A dead thread is not parked — nothing to count — but the
            // outcome is still unknowable (§6.6).
            tracing::warn!(
                target: "custos::engine",
                "executor thread died before reporting; outcome unknown (§6.6)"
            );
            ActionOutcome::OutcomeUnknown
        }
    }
}

/// Builtin exec executor: runs the parameterized argv under the same
/// controlled-execution rules as the Exec Probe (user ruling 2026-10-29:
/// builtin base adapters). A plugin-dispatched action (no `program`)
/// reaching this executor is a wiring gap, not a retryable failure (C21).
pub struct BuiltinExecExecutor;

impl ActionExecutor for BuiltinExecExecutor {
    fn execute(&self, action: &ActionSpec, timeout: Duration) -> ActionOutcome {
        let Some(program) = &action.program else {
            tracing::warn!(
                target: "custos::engine",
                "argv executor received an action without `program`; \
                 plugin operations need the plugin-routing executor"
            );
            return ActionOutcome::NotStarted;
        };
        let argv: Vec<String> = std::iter::once(program.clone())
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

/// Routes actions to their executor: plugin operations (`plugin` +
/// `operation_id`) go to the named plugin's supervisor over the `custos/v1`
/// protocol; argv actions fall through to the builtin exec executor
/// (docs/guide/plugins.md §5.5: the host dispatches by operation id, the
/// core owns execution and its budgets).
pub struct PluginRoutingExecutor {
    builtin: BuiltinExecExecutor,
    plugins: HashMap<String, std::sync::Arc<custos_plugin_host::PluginSupervisor>>,
}

impl PluginRoutingExecutor {
    /// One supervisor per declared `[[plugins]]` entry.
    pub fn new(
        plugins: HashMap<String, std::sync::Arc<custos_plugin_host::PluginSupervisor>>,
    ) -> Self {
        Self {
            builtin: BuiltinExecExecutor,
            plugins,
        }
    }
}

impl ActionExecutor for PluginRoutingExecutor {
    fn execute(&self, action: &ActionSpec, timeout: Duration) -> ActionOutcome {
        let Some(operation_id) = &action.operation_id else {
            return self.builtin.execute(action, timeout);
        };
        let plugin = action.plugin.as_deref().unwrap_or_default();
        let Some(supervisor) = self.plugins.get(plugin) else {
            // Config validation rejects unknown plugin references; reaching
            // here means a wiring layer skipped it — permanent for the plan
            // (C21), never a retryable transient.
            tracing::warn!(
                target: "custos::engine",
                "action references undeclared plugin `{plugin}`; refusing to start"
            );
            return ActionOutcome::NotStarted;
        };
        let _ = timeout; // deadline enforcement: watchdog + plugin request budget
        let params = custos_plugin_api::OperationParams {
            operation_id: operation_id.clone(),
            resource_id: String::new(),
            run_id: None,
            args: Default::default(),
        };
        let params = serde_json::to_value(&params).unwrap_or_default();
        match supervisor.call(custos_plugin_api::Method::ExecuteRecoveryAction, params) {
            Ok(result) => {
                let succeeded = result
                    .get("succeeded")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                // The plugin ran and reported: `succeeded: false` is its
                // verdict about the backend — retryable within the ladder.
                if succeeded {
                    ActionOutcome::Succeeded
                } else {
                    ActionOutcome::Failed
                }
            }
            // Plugin crashed or degraded mid-call: whether the action ran
            // is unknowable (§6.6) — never blindly repeat.
            Err(custos_plugin_host::HostError::Unavailable) => ActionOutcome::OutcomeUnknown,
            Err(custos_plugin_host::HostError::Rpc { code, .. })
                if code == custos_plugin_api::RpcError::TIMED_OUT =>
            {
                ActionOutcome::TimedOut
            }
            // Unknown operation, malformed reply, protocol violation: the
            // action never legitimately started (C21).
            Err(_) => ActionOutcome::NotStarted,
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
        ActionSpec::argv("true", &[])
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

    /// The leak counter is process-global; these two tests assert against it
    /// and must not observe each other's parked workers.
    static LEAK_COUNTER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn watchdog_declares_unknown_when_the_executor_never_returns() {
        let _order = LEAK_COUNTER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = std::sync::Mutex::new(gate_rx);
        let executor: Arc<dyn ActionExecutor> = Arc::new(move |_: &ActionSpec, _: Duration| {
            let _ = gate_rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
            ActionOutcome::Succeeded
        });
        let started = std::time::Instant::now();
        let baseline = leaked_watchdog_workers();
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
        // The worker is parked on the gated executor: exactly one leaked
        // worker is outstanding (2026-11-11 audit P0-2).
        assert_eq!(
            leaked_watchdog_workers(),
            baseline + 1,
            "a hung executor must hold exactly one worker"
        );
        // Release the parked worker; the late result must be dropped, not
        // delivered or panicked on — and the leak counter must clear.
        drop(gate_tx);
        let cleared = std::time::Instant::now();
        while leaked_watchdog_workers() > baseline {
            assert!(
                cleared.elapsed() < Duration::from_secs(5),
                "released worker never left the leak counter"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A worker that panics mid-execute drops `tx` without sending: the
    /// channel disconnects, the thread is gone — dead, not parked — so the
    /// leak counter must stay at baseline (2026-11-12 Codex audit).
    #[test]
    fn watchdog_does_not_count_a_panicked_executor_as_leaked() {
        let _order = LEAK_COUNTER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let baseline = leaked_watchdog_workers();
        let executor: Arc<dyn ActionExecutor> = Arc::new(|_: &ActionSpec, _: Duration| {
            panic!("executor exploded before reporting");
        });
        let outcome = execute_with_watchdog(executor, &action(), Duration::from_millis(50));
        assert_eq!(outcome, ActionOutcome::OutcomeUnknown);
        assert_eq!(
            leaked_watchdog_workers(),
            baseline,
            "a panicked executor thread is dead, not parked"
        );
    }

    #[test]
    fn watchdog_leaks_no_worker_when_the_executor_honors_its_deadline() {
        let _order = LEAK_COUNTER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let baseline = leaked_watchdog_workers();
        let executor: Arc<dyn ActionExecutor> =
            Arc::new(|_: &ActionSpec, _: Duration| ActionOutcome::Succeeded);
        let outcome = execute_with_watchdog(executor, &action(), Duration::from_millis(50));
        assert_eq!(outcome, ActionOutcome::Succeeded);
        assert_eq!(
            leaked_watchdog_workers(),
            baseline,
            "a timely executor must not park a worker past its deadline"
        );
    }
}

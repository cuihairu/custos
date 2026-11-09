//! Plugin supervisor: keeps a plugin process long-lived, watches its
//! health on every access, and restarts it under a bounded policy
//! (docs/guide/plugins.md §4.5).
//!
//! Restarts are lazy and engine-driven — there is no background timer
//! thread. Every [`PluginSupervisor::call`] first checks liveness; a dead
//! session is respawned (spawn + handshake + capability validation) as
//! long as the crash budget allows. A plugin that crashes more than
//! `max_restarts` times inside the rolling `window` is marked
//! **unrecoverable**: further calls fail fast without touching the
//! process, and [`PluginSupervisor::status`] reports the degraded state so
//! the engine can mark the affected resources unprobeable and notify.
//! The host process itself is never taken down by any of this.

use crate::session::{HostError, PluginConfig, PluginSession};
use crate::validate_capabilities;
use custos_plugin_api::Method;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How aggressively a crashed plugin may be respawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPolicy {
    /// Restarts allowed inside the rolling window.
    pub max_restarts: usize,
    /// Length of the rolling window.
    pub window: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            window: Duration::from_secs(60),
        }
    }
}

/// Liveness snapshot of a supervised plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// A healthy session is up; `restarts` counts crash recoveries.
    Running { restarts: usize },
    /// Never started, or shut down cleanly.
    Stopped,
    /// Crash budget exhausted — the plugin is unrecoverable until the
    /// window expires. Callers map this to "resources unprobeable" plus a
    /// fault notification.
    Degraded { reason: String },
}

struct SupervisorState {
    session: Option<Arc<PluginSession>>,
    /// Crash timestamps inside the rolling window.
    crashes: VecDeque<Instant>,
    /// Every crash ever seen (monotonic, never evicted).
    crashes_total: usize,
    /// stderr tail of the last dead session — the plugin's last words,
    /// kept for incident reports across restarts.
    dying_stderr: Option<String>,
    unrecoverable: Option<String>,
}

/// A long-lived plugin under a restart policy.
pub struct PluginSupervisor {
    name: String,
    config: PluginConfig,
    policy: RestartPolicy,
    state: Mutex<SupervisorState>,
    /// Serializes respawns: held (while `state` is NOT held) for the whole
    /// spawn + handshake, so the multi-second plugin I/O never pins the
    /// bookkeeping lock that `status()` and the engine tick read through.
    /// Lock order is always `spawn_gate` → `state`.
    spawn_gate: Mutex<()>,
}

impl PluginSupervisor {
    pub fn new(name: impl Into<String>, config: PluginConfig, policy: RestartPolicy) -> Self {
        Self {
            name: name.into(),
            config,
            policy,
            state: Mutex::new(SupervisorState {
                session: None,
                crashes: VecDeque::new(),
                crashes_total: 0,
                dying_stderr: None,
                unrecoverable: None,
            }),
            spawn_gate: Mutex::new(()),
        }
    }

    /// The plugin's registry name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Spawns the plugin for the first time and runs the full handshake.
    pub fn start(&self) -> Result<(), HostError> {
        {
            let mut state = lock(&self.state);
            if live_session(&state).is_some() {
                return Ok(());
            }
            self.enter_gate(&mut state)?;
        }
        let _spawn_gate = lock(&self.spawn_gate);
        let mut state = lock(&self.state);
        if live_session(&state).is_some() {
            return Ok(());
        }
        self.enter_gate(&mut state)?;
        drop(state);
        self.spawn_with_budget()
    }

    /// Current liveness.
    pub fn status(&self) -> PluginStatus {
        let state = lock(&self.state);
        if let Some(reason) = &state.unrecoverable {
            return PluginStatus::Degraded {
                reason: reason.clone(),
            };
        }
        match &state.session {
            Some(session) if session.is_running() => PluginStatus::Running {
                restarts: state.crashes_total,
            },
            _ => PluginStatus::Stopped,
        }
    }

    /// Calls a method on the plugin, transparently restarting a dead
    /// session within the crash budget. Returns
    /// [`HostError::Unavailable`] immediately when the plugin is
    /// degraded, without spawning anything.
    pub fn call(
        &self,
        method: Method,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        let session = self.healthy_session()?;
        match session.call(method, params) {
            Err(HostError::Unavailable) => {
                // A live plugin may also deliver Unavailable as a
                // declarative RPC error — its verdict about its backend,
                // not its death. Pass it through untouched: burning the
                // crash budget here would mark a healthy process
                // unrecoverable (C27).
                if session.is_running() {
                    return Err(HostError::Unavailable);
                }
                // The session died under us: absorb the death into the
                // crash budget and surface it; the NEXT call decides
                // whether a restart is still affordable.
                self.absorb_crash();
                Err(HostError::Unavailable)
            }
            other => other,
        }
    }

    /// Retained stderr diagnostics: the current session plus the last
    /// dead session's final output.
    pub fn stderr_tail(&self) -> String {
        let state = lock(&self.state);
        let current = state
            .session
            .as_ref()
            .map(|session| session.stderr_tail())
            .unwrap_or_default();
        match &state.dying_stderr {
            Some(dead) if current.is_empty() => dead.clone(),
            Some(dead) => format!("{dead}\n{current}"),
            None => current,
        }
    }

    /// Protocol faults observed on the current (or last) session.
    pub fn faults(&self) -> Vec<String> {
        lock(&self.state)
            .session
            .as_ref()
            .map(|session| session.faults())
            .unwrap_or_default()
    }

    /// Graceful shutdown; resets the crash budget so the supervisor can
    /// be started again later. Takes the spawn gate first (original
    /// semantics: an in-flight respawn finishes before the teardown) so
    /// its install cannot land a session after the shutdown.
    pub fn shutdown(&self) {
        let _spawn_gate = lock(&self.spawn_gate);
        let mut state = lock(&self.state);
        if let Some(session) = state.session.take() {
            let _ = session.shutdown();
        }
        state.crashes.clear();
        state.unrecoverable = None;
    }

    /// Hands out a session that is running right now, respawning within
    /// the budget when needed. The respawn I/O runs on the spawn gate
    /// with no lock held: concurrent callers queue on the gate and
    /// re-check the session instead of pinning `state`, so `status()`
    /// and the engine tick never wait out a plugin handshake.
    fn healthy_session(&self) -> Result<Arc<PluginSession>, HostError> {
        {
            let mut state = lock(&self.state);
            self.enter_gate(&mut state)?;
            if let Some(session) = live_session(&state) {
                return Ok(session);
            }
        }
        let _spawn_gate = lock(&self.spawn_gate);
        // Retire the dead session with the lock released — capturing its
        // dying words polls stderr for up to 100ms, so the guard must be
        // gone by then.
        let dead = {
            let mut state = lock(&self.state);
            self.enter_gate(&mut state)?;
            // A concurrent caller may have respawned while we queued on
            // the gate: reuse its work instead of spawning again.
            if let Some(session) = live_session(&state) {
                return Ok(session);
            }
            state.session.take()
        };
        let dead_words = dead.as_deref().and_then(dying_words);
        {
            let mut state = lock(&self.state);
            self.record_crash_locked(&mut state, dead.is_some(), dead_words);
        }
        self.spawn_with_budget()?;
        lock(&self.state)
            .session
            .clone()
            .ok_or(HostError::Unavailable)
    }

    /// Fails fast while the plugin is unrecoverable — unless the crash
    /// window has fully expired, in which case the budget resets and a
    /// restart is allowed again.
    fn enter_gate(&self, state: &mut SupervisorState) -> Result<(), HostError> {
        if state.unrecoverable.is_some() {
            let now = Instant::now();
            let all_expired = state
                .crashes
                .iter()
                .all(|at| now.duration_since(*at) > self.policy.window);
            if all_expired {
                state.crashes.clear();
                state.unrecoverable = None;
            } else {
                return Err(HostError::Unavailable);
            }
        }
        Ok(())
    }

    /// Spawns a fresh session within the crash budget. Budget check and
    /// result bookkeeping run under short state-lock sections; the process
    /// spawn and the two handshake RPC round trips run with no lock held.
    fn spawn_with_budget(&self) -> Result<(), HostError> {
        {
            let mut state = lock(&self.state);
            let now = Instant::now();
            while state
                .crashes
                .front()
                .is_some_and(|at| now.duration_since(*at) > self.policy.window)
            {
                state.crashes.pop_front();
            }
            if state.crashes.len() >= self.policy.max_restarts {
                state.unrecoverable = Some(format!(
                    "plugin `{}` crashed {} times within {:?}; giving up until the window expires",
                    self.name,
                    state.crashes.len(),
                    self.policy.window
                ));
                return Err(HostError::Unavailable);
            }
        }
        let spawned = Self::spawn_and_handshake(&self.config);
        let mut state = lock(&self.state);
        match spawned {
            Ok(session) => {
                state.session = Some(Arc::new(session));
                Ok(())
            }
            Err((e, dying_stderr)) => {
                if dying_stderr.is_some() {
                    state.dying_stderr = dying_stderr;
                }
                state.crashes.push_back(Instant::now());
                state.crashes_total += 1;
                Err(e)
            }
        }
    }

    /// Spawns and handshakes; on failure returns the error together with
    /// the failed session's stderr tail (its last words), so callers that
    /// already hold the state lock can record it without re-locking.
    fn spawn_and_handshake(
        config: &PluginConfig,
    ) -> Result<PluginSession, (HostError, Option<String>)> {
        let fail = |session: &PluginSession, error: HostError| Err((error, dying_words(session)));
        let session = match PluginSession::start(config) {
            Ok(session) => session,
            Err(e) => return Err((e, None)),
        };
        if let Err(e) = session.initialize() {
            return fail(&session, e);
        }
        let capabilities = match session.capabilities() {
            Ok(capabilities) => capabilities,
            Err(e) => return fail(&session, e),
        };
        if let Err(e) = validate_capabilities(&capabilities, &[]) {
            return fail(&session, e);
        }
        Ok(session)
    }

    fn absorb_crash(&self) {
        let dead = lock(&self.state).session.take();
        // Only a dead session burns the budget (C27): the same error can
        // surface while the process is alive, and `record_crash_locked`
        // used to count any taken session — live ones included.
        let really_dead = dead.as_ref().is_some_and(|session| !session.is_running());
        // Dying words are captured before re-locking: the poll sleeps up
        // to 100ms.
        let dead_words = if really_dead {
            dead.as_deref().and_then(dying_words)
        } else {
            None
        };
        let mut state = lock(&self.state);
        self.record_crash_locked(&mut state, really_dead, dead_words);
    }

    /// Crash bookkeeping under the state lock; the dying words are
    /// captured by the caller while no lock is held.
    fn record_crash_locked(
        &self,
        state: &mut SupervisorState,
        dead: bool,
        dead_words: Option<String>,
    ) {
        if let Some(tail) = dead_words {
            state.dying_stderr = Some(tail);
        }
        if dead {
            state.crashes.push_back(Instant::now());
            state.crashes_total += 1;
        }
    }
}

/// The current session, if it is running right now.
fn live_session(state: &SupervisorState) -> Option<Arc<PluginSession>> {
    state
        .session
        .as_ref()
        .filter(|session| session.is_running())
        .cloned()
}

/// The last stderr output of a dying session. A process's final stderr
/// line races its exit, so wait briefly (bounded) for the drain thread to
/// capture it before the session is dropped.
fn dying_words(session: &PluginSession) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut tail = session.stderr_tail();
    while tail.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
        tail = session.stderr_tail();
    }
    (!tail.is_empty()).then_some(tail)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_bounded() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.max_restarts, 3);
        assert_eq!(policy.window, Duration::from_secs(60));
    }
}

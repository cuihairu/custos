//! Engine concern: tick orchestration, waiting-run resume and escalation.

use super::*;

impl Engine {
    /// One observation tick: probe every due resource, update health
    /// snapshots, and trigger recovery for resources that crossed their
    /// failure threshold.
    pub fn tick(&mut self) -> Vec<RecoveryRun> {
        let now = self.clock.now_unix_ms();
        let mut finished = Vec::new();
        for id in self.probe_order.clone() {
            let interval_ms = self.managed[&id].interval_ms;
            if !self.tracker.is_due(&id, interval_ms, now) {
                continue;
            }
            self.probe_one(&id, now);
            if let Ok(run) = self.try_recover(&id) {
                finished.push(run);
            }
        }
        // Waiting runs restored from a previous process re-enter the cycle
        // here: dependencies are re-checked and the run is either re-suspended
        // under the right key or driven to a terminal state.
        for run in std::mem::take(&mut self.resume_queue) {
            self.redrive(run, &mut finished);
        }
        // §3.6 dedup: wake every waiter whose dependency's health changed.
        self.wake_ready_waiters(&mut finished);
        finished
    }
    /// Re-drives a `Waiting` run: `Waiting -> Planning` is the only legal
    /// resume (§6.1); `drive` re-checks dependencies first.
    pub(super) fn redrive(&mut self, run: RecoveryRun, out: &mut Vec<RecoveryRun>) {
        let id = run.resource_id.clone();
        if run.status != RecoveryRunStatus::Waiting {
            return; // safety: nothing to resume
        }
        if self.active_runs.contains(&id) {
            // The run was already popped from its queue; re-queue it so it
            // is tried again next tick instead of being orphaned forever.
            self.resume_queue.push(run);
            return;
        }
        // The wait condition may have cleared itself.
        if self.managed[&id].snapshot.status == HealthStatus::Healthy {
            self.finish_cancelled(run);
            return;
        }
        let fault = run
            .diagnosis
            .as_ref()
            .map(|d| d.fault_code)
            .unwrap_or(FaultCode::UnknownFailure);
        let mut step = (run.action_attempts.len() + run.verification_results.len()) as u32;
        self.active_runs.insert(id.clone());
        let finished = self.drive(run, &id, fault, &mut step);
        self.active_runs.remove(&id);
        out.push(finished);
    }
    /// Cancels a waiting run whose resource is already healthy (the wait
    /// condition cleared itself).
    pub(super) fn finish_cancelled(&mut self, mut run: RecoveryRun) {
        run.transition_to(RecoveryRunStatus::Cancelled).ok();
        self.store.save_run(&run);
        self.wait_cycles.remove(&run.id);
        tracing::info!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = run.resource_id.as_str(),
            "waiting run cancelled: resource healthy again"
        );
    }
    /// Wakes waiters whose dependency turned healthy (health change →
    /// re-evaluate). A dependency that is due for another attempt is retried
    /// here — parked dependents make its recovery urgent even when its own
    /// probe interval has not elapsed. Escalates waiters on a dependency
    /// that can never recover (no plans) or that exhausts their wait budget.
    pub(super) fn wake_ready_waiters(&mut self, out: &mut Vec<RecoveryRun>) {
        let deps: Vec<ResourceId> = self
            .waiters
            .iter()
            .filter(|(_, parked)| !parked.is_empty())
            .map(|(dep, _)| dep.clone())
            .collect();
        for dep in deps {
            let now = self.clock.now_unix_ms();
            self.probe_one(&dep, now);
            let mut healthy = self.managed[&dep].snapshot.status == HealthStatus::Healthy;
            if !healthy {
                let cooling_down = now_lt(now, self.managed[&dep].cooldown_until_ms);
                let has_plans = !self.managed[&dep].resource.recovery_policy.plans.is_empty();
                if !has_plans {
                    // The dependency cannot recover by itself: stop holding
                    // the dependents hostage (§15 rule 5 already blocked
                    // their upstream actions).
                    self.escalate_waiters(&dep, out, "dependency has no recovery plans");
                    continue;
                }
                if !cooling_down && !self.active_runs.contains(&dep) {
                    // Retry the dependency now (its own budgets still apply),
                    // then re-evaluate on the freshest evidence. A refusal
                    // must not vanish (C5): the parked dependents keep
                    // waiting/burning cycles, so the reason is reported.
                    if let Err(err) = self.try_recover(&dep) {
                        tracing::warn!(
                            target: "custos::engine",
                            dependency = dep.as_str(),
                            error = %err,
                            "dependency retry refused; parked dependents keep waiting"
                        );
                    }
                    self.probe_one(&dep, self.clock.now_unix_ms());
                    healthy = self.managed[&dep].snapshot.status == HealthStatus::Healthy;
                    if !healthy {
                        // The retry burned one wait cycle for every parked
                        // dependent; over-budget ones escalate.
                        self.burn_wait_cycles(&dep, out);
                    }
                }
            }
            if healthy {
                let waiters = self.waiters.remove(&dep).unwrap_or_default();
                tracing::debug!(
                    target: "custos::engine",
                    dependency = dep.as_str(),
                    count = waiters.len(),
                    "dependency healthy: waking waiters"
                );
                for run in waiters {
                    self.wait_cycles.remove(&run.id);
                    self.redrive(run, out);
                }
            }
        }
    }
    /// Escalates every waiter parked on `dep` (dependency hopeless).
    pub(super) fn escalate_waiters(
        &mut self,
        dep: &ResourceId,
        out: &mut Vec<RecoveryRun>,
        why: &str,
    ) {
        for mut run in self.waiters.remove(dep).unwrap_or_default() {
            let id = run.resource_id.clone();
            self.wait_cycles.remove(&run.id);
            tracing::warn!(
                target: "custos::engine",
                run = run.id.as_str(),
                dependency = dep.as_str(),
                "{why}: escalating waiter"
            );
            self.finish_escalated(&mut run, &id);
            out.push(run);
        }
    }
    /// Consumes one wait cycle for every waiter parked on `dep`; escalates
    /// the ones past [`MAX_WAIT_CYCLES`].
    pub(super) fn burn_wait_cycles(&mut self, dep: &ResourceId, out: &mut Vec<RecoveryRun>) {
        let mut still_parked = Vec::new();
        for mut run in self.waiters.remove(dep).unwrap_or_default() {
            let cycles = self.wait_cycles.entry(run.id.clone()).or_insert(0);
            *cycles += 1;
            if *cycles > MAX_WAIT_CYCLES {
                self.wait_cycles.remove(&run.id);
                let id = run.resource_id.clone();
                tracing::warn!(
                    target: "custos::engine",
                    run = run.id.as_str(),
                    dependency = dep.as_str(),
                    cycles = MAX_WAIT_CYCLES,
                    "wait budget exhausted: escalating waiter"
                );
                self.finish_escalated(&mut run, &id);
                out.push(run);
            } else {
                still_parked.push(run);
            }
        }
        if !still_parked.is_empty() {
            self.waiters.insert(dep.clone(), still_parked);
        }
    }
    /// Crash recovery (§6.6, §7.3): attempts persisted as `Executing` have
    /// unknown outcomes; mark them so, never blindly re-execute. Returns the
    /// number of affected runs.
    pub fn resume_unfinished(&mut self) -> usize {
        let unfinished = self.store.unfinished_runs();
        let mut affected = 0;
        for mut run in unfinished {
            if run.status == RecoveryRunStatus::Waiting {
                // A waiting run is not an unknown outcome: its dependency
                // key is re-derived when it re-enters the cycle on the next
                // tick (dependencies re-probed, then re-suspended or driven).
                self.resume_queue.push(run);
                continue;
            }
            let resource_id = run.resource_id.clone();
            let now = self.clock.now_unix_ms();
            let mut changed = false;
            for attempt in &mut run.action_attempts {
                if attempt.outcome.is_none() {
                    attempt.outcome = Some(ActionOutcome::OutcomeUnknown);
                    attempt.finished_at_unix_ms = Some(now);
                    self.store.save_attempt(&run.id, attempt);
                    changed = true;
                }
            }
            if changed {
                // Crashed mid-execution: the persisted attempt had no
                // outcome. `OutcomeUnknown` is the legal transition from
                // `Executing` (§6.6); never blindly re-execute.
                run.status = RecoveryRunStatus::OutcomeUnknown;
                run.updated_at_unix_ms = now;
                self.store.save_run(&run);
                self.store.emit(EngineEvent::OutcomeUnknown {
                    run_id: run.id.clone(),
                    resource_id: resource_id.clone(),
                    at_unix_ms: now,
                });
                self.emit_notification(
                    custos_model::EventType::RecoveryOutcomeUnknown,
                    custos_model::Severity::Warning,
                    "resumed unfinished run: action outcome may be unknown".to_owned(),
                    &resource_id,
                    Some(&run.id),
                    run.incident_id.as_ref(),
                );
                tracing::warn!(
                    target: "custos::engine",
                    run = run.id.as_str(),
                    resource = resource_id.as_str(),
                    "resumed unfinished run: outcome may be unknown, not re-executing"
                );
                affected += 1;
            } else if !run.status.is_terminal() {
                // Crashed before or between actions (Planning/Diagnosing/
                // Verifying, no unknown outcome): this is NOT an unknown
                // outcome. Re-drive it through the normal cycle rather than
                // declaring it terminal and never re-entering.
                self.resume_queue.push(run);
            }
        }
        affected
    }
}

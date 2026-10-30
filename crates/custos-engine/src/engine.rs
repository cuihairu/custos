//! The recovery engine: drives the RecoveryRun state machine end to end
//! (docs/guide/recovery-engine.md §6).
//!
//! Order of operations per failure: diagnose → resolve dependencies (recurse
//! into their recovery, then re-probe them) → select an applicable candidate
//! plan → persist the attempt as `Executing` → execute → verify by probing,
//! never by exit code → re-diagnose if the fault remains → next plan →
//! escalate when plans or budgets are exhausted.

use crate::budget::{BudgetExceeded, BudgetState, RecoveryBudget};
use crate::clock::Clock;
use crate::events::{EngineEvent, RunStore};
use crate::executor::ActionExecutor;
use custos_config::ValidatedConfig;
use custos_model::ids::{ActionId, IncidentId, RecoveryRunId};
use custos_model::{
    ActionAttempt, ActionOutcome, Diagnosis, FaultCode, HealthStatus, RecoveryPlan, RecoveryRun,
    RecoveryRunStatus, Resource, ResourceHealthSnapshot, ResourceId, VerificationResult,
};
use custos_probe::{Probe, ProbeDueTracker, ProbeReport};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum RecoverError {
    #[error("recovery already active for `{0}` (per-resource mutex, §3.7)")]
    ResourceBusy(ResourceId),
    #[error("budget exceeded: {0}")]
    Budget(#[from] BudgetExceeded),
    #[error("nothing to do: not unhealthy, cooling down, disabled or no plans")]
    Stopped,
}

struct ManagedResource {
    resource: Resource,
    snapshot: ResourceHealthSnapshot,
    cooldown_until_ms: u64,
    interval_ms: u64,
}

pub struct Engine {
    managed: HashMap<ResourceId, ManagedResource>,
    /// Topological order (dependencies first).
    probe_order: Vec<ResourceId>,
    probes: HashMap<ResourceId, Box<dyn Probe>>,
    tracker: ProbeDueTracker,
    executor: Box<dyn ActionExecutor>,
    store: Arc<dyn RunStore>,
    clock: Arc<dyn Clock>,
    budget: RecoveryBudget,
    budget_state: BudgetState,
    /// Active-mode keys that switch Conditional dependencies on/off.
    pub active_modes: HashSet<String>,
    active_runs: HashSet<ResourceId>,
    /// Runs suspended in `Waiting`, keyed by the dependency they await
    /// (§3.6 dependency dedup): every dependent waits on the SAME recovery
    /// result and is re-evaluated when that dependency's health changes.
    waiters: HashMap<ResourceId, Vec<RecoveryRun>>,
    /// How many times each waiting run has been re-suspended; bounded so a
    /// permanently failing dependency cannot hold dependents forever.
    wait_cycles: HashMap<RecoveryRunId, u32>,
    /// Waiting runs restored from the store at startup; re-driven (and
    /// re-keyed) on the first tick.
    resume_queue: Vec<RecoveryRun>,
    seq: u64,
}

/// A dependent gives up waiting after this many re-suspensions on the same
/// dependency cycle (escalation takes over).
const MAX_WAIT_CYCLES: u32 = 5;

/// Increments the run-local step counter and returns its new value.
fn bump(step: &mut u32) -> u32 {
    *step += 1;
    *step
}

/// `now < until` with a default of "not cooling down" when until is 0.
fn now_lt(now: u64, until: u64) -> bool {
    until > 0 && now < until
}

impl Engine {
    pub fn new(
        config: &ValidatedConfig,
        probes: HashMap<ResourceId, Box<dyn Probe>>,
        executor: Box<dyn ActionExecutor>,
        store: Arc<dyn RunStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let now = clock.now_unix_ms();
        let mut managed = HashMap::new();
        for resource in &config.resources {
            let interval_ms = resource
                .config
                .get("interval_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(10_000);
            managed.insert(
                resource.id.clone(),
                ManagedResource {
                    resource: resource.clone(),
                    snapshot: ResourceHealthSnapshot::new(resource.id.clone(), now),
                    cooldown_until_ms: 0,
                    interval_ms,
                },
            );
        }
        Self {
            probe_order: config.graph.topological_order(),
            managed,
            probes,
            tracker: ProbeDueTracker::default(),
            executor,
            store,
            clock,
            budget: RecoveryBudget::default(),
            budget_state: BudgetState::default(),
            active_modes: HashSet::new(),
            active_runs: HashSet::new(),
            waiters: HashMap::new(),
            wait_cycles: HashMap::new(),
            resume_queue: Vec::new(),
            seq: 0,
        }
    }

    pub fn set_budget(&mut self, budget: RecoveryBudget) {
        self.budget = budget;
    }

    pub fn snapshot(&self, id: &ResourceId) -> Option<&ResourceHealthSnapshot> {
        self.managed.get(id).map(|m| &m.snapshot)
    }

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
    fn redrive(&mut self, run: RecoveryRun, out: &mut Vec<RecoveryRun>) {
        let id = run.resource_id.clone();
        if run.status != RecoveryRunStatus::Waiting || self.active_runs.contains(&id) {
            return; // safety: nothing to resume
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
    fn finish_cancelled(&mut self, mut run: RecoveryRun) {
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
    fn wake_ready_waiters(&mut self, out: &mut Vec<RecoveryRun>) {
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
                    // then re-evaluate on the freshest evidence.
                    let _ = self.try_recover(&dep);
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
    fn escalate_waiters(&mut self, dep: &ResourceId, out: &mut Vec<RecoveryRun>, why: &str) {
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
    fn burn_wait_cycles(&mut self, dep: &ResourceId, out: &mut Vec<RecoveryRun>) {
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

    /// Probes one resource and applies jitter-suppressed threshold logic.
    pub fn probe_one(&mut self, id: &ResourceId, now_unix_ms: u64) -> ProbeReport {
        let (report, thresholds) = {
            let managed = &self.managed[id];
            let probe = self
                .probes
                .get(id)
                .expect("probe wired for every resource at construction");
            let report = probe.probe(&managed.resource);
            // Evidence is persisted for every probe (§7.2 probe_results):
            // diagnosis must be re-derivable from stored facts.
            self.store.save_probe_result(&custos_model::Evidence {
                resource_id: id.clone(),
                probe_name: probe.name().to_owned(),
                succeeded: report.succeeded,
                observation: report.observation.clone(),
                collected_at_unix_ms: now_unix_ms,
            });
            (
                report,
                (
                    managed.resource.health_policy.failure_threshold,
                    managed.resource.health_policy.success_threshold,
                ),
            )
        };
        let snapshot = &mut self.managed.get_mut(id).unwrap().snapshot;
        snapshot.record_probe(report.succeeded, thresholds.0, thresholds.1, now_unix_ms);
        tracing::debug!(
            target: "custos::engine",
            resource = id.as_str(),
            succeeded = report.succeeded,
            observation = %report.observation,
            "probe collected"
        );
        report
    }

    /// Starts a recovery run unless one is already active (per-resource
    /// mutex, docs/guide/resources.md §3.7) or the budget refuses.
    pub fn try_recover(&mut self, id: &ResourceId) -> Result<RecoveryRun, RecoverError> {
        let now = self.clock.now_unix_ms();
        {
            let managed = &self.managed[id];
            if !managed.resource.enabled
                || managed.snapshot.status != HealthStatus::Unhealthy
                || now < managed.cooldown_until_ms
                || managed.resource.recovery_policy.plans.is_empty()
            {
                return Err(RecoverError::Stopped);
            }
        }
        if self.active_runs.contains(id) {
            return Err(RecoverError::ResourceBusy(id.clone()));
        }
        self.budget_state
            .check_start(id, &self.budget, self.active_runs.len())?;

        self.active_runs.insert(id.clone());
        let run = self.recover(id);
        self.active_runs.remove(id);
        Ok(run)
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}-{}", self.seq)
    }

    /// The full recovery cycle for one resource: open the run, diagnose,
    /// then hand over to [`Engine::drive`].
    fn recover(&mut self, id: &ResourceId) -> RecoveryRun {
        let now = self.clock.now_unix_ms();
        let run_id = RecoveryRunId::new(self.next_id("run"));
        let incident = IncidentId::new(self.next_id("inc"));
        let mut run = RecoveryRun::new(run_id, id.clone(), "failure threshold reached", now);
        run.incident_id = Some(incident.clone());
        run.transition_to(RecoveryRunStatus::ProblemDetected).ok();
        self.store.save_run(&run);
        self.store.emit(EngineEvent::RecoveryStarted {
            run_id: run.id.clone(),
            resource_id: id.clone(),
            at_unix_ms: now,
        });

        // Diagnose from the freshest evidence.
        run.transition_to(RecoveryRunStatus::Diagnosing).ok();
        let fault = {
            let diagnosis = self.diagnose(id, now);
            let fault = diagnosis.fault_code;
            self.store.save_diagnosis(Some(&run.id), &diagnosis, now);
            self.store.save_incident(&custos_model::Incident::open(
                incident,
                id.clone(),
                fault,
                diagnosis.summary.clone(),
                now,
            ));
            run.diagnosis = Some(diagnosis);
            fault
        };
        self.store.save_run(&run);
        tracing::info!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = id.as_str(),
            fault = ?fault,
            "recovery run started"
        );

        let mut step = 0u32;
        self.drive(run, id, fault, &mut step)
    }

    /// Continues a run from `Planning`: dependency resolution, plan loop,
    /// verification, terminal state. Also the resume path for `Waiting`
    /// runs (`Waiting -> Planning` is legal, §6.1).
    fn drive(
        &mut self,
        mut run: RecoveryRun,
        id: &ResourceId,
        fault: FaultCode,
        step: &mut u32,
    ) -> RecoveryRun {
        // Position the run for dependency resolution; every attempt
        // re-resolves dependencies before it executes (§3.6, §16.3 case 5:
        // a dependency re-faulting mid-recovery blocks the next upstream
        // action).
        run.transition_to(RecoveryRunStatus::Planning).ok();
        run.transition_to(RecoveryRunStatus::ResolvingDependencies)
            .ok();
        self.store.save_run(&run);

        // Select and execute applicable plans, re-diagnosing between them.
        let plans = self.managed[id].resource.recovery_policy.plans.clone();
        let policy_max = self.managed[id]
            .resource
            .recovery_policy
            .max_attempts
            .unwrap_or(u32::MAX);

        for plan in &plans {
            if !plan.applies_to(fault) {
                continue;
            }
            let attempts_allowed = plan.max_attempts.min(policy_max);
            for attempt_no in 1..=attempts_allowed {
                // The state machine always passes through Planning before an
                // execution (§6.1).
                run.transition_to(RecoveryRunStatus::Planning).ok();
                run.transition_to(RecoveryRunStatus::ResolvingDependencies)
                    .ok();
                self.store.save_run(&run);
                // Fresh dependency check before EVERY attempt: state must be
                // re-read, never assumed (§2.3 rules 4/5). Stops the drive on
                // escalation or suspension.
                if !self.dependencies_satisfied(id, &mut run, step) {
                    if run.status != RecoveryRunStatus::Waiting {
                        self.apply_cooldown(id);
                    }
                    return run;
                }
                self.store.save_step(
                    &run.id,
                    bump(step),
                    "resolving_dependencies",
                    "all binding dependencies healthy",
                    self.clock.now_unix_ms(),
                );
                if self.budget_gate(id, &run, plan).is_err() {
                    self.finish_escalated(&mut run, id);
                    self.apply_cooldown(id);
                    return run;
                }
                self.store.save_step(
                    &run.id,
                    bump(step),
                    "executing",
                    &format!("plan `{}` attempt {attempt_no}", plan.plan_id),
                    self.clock.now_unix_ms(),
                );
                self.execute_attempt(id, plan, attempt_no, &mut run);
                run.transition_to(RecoveryRunStatus::Verifying).ok();
                if self.verify(id, plan, &mut run) {
                    run.transition_to(RecoveryRunStatus::Recovered).ok();
                    self.store.save_step(
                        &run.id,
                        bump(step),
                        "recovered",
                        &format!("plan `{}` verified", plan.plan_id),
                        self.clock.now_unix_ms(),
                    );
                    self.close_incident_of(&run);
                    let run_id = run.id.clone();
                    let at = self.clock.now_unix_ms();
                    self.finish_run(
                        &mut run,
                        EngineEvent::RecoverySucceeded {
                            run_id,
                            resource_id: id.clone(),
                            at_unix_ms: at,
                        },
                    );
                    tracing::info!(
                        target: "custos::engine",
                        run = run.id.as_str(),
                        resource = id.as_str(),
                        plan = plan.plan_id.as_str(),
                        "recovery succeeded and verified"
                    );
                    self.apply_cooldown(id);
                    return run;
                }
                // Fault remains or changed: re-diagnose before the next move
                // (docs/guide/recovery-engine.md §6.4). OutcomeUnknown never
                // re-executes blindly (§6.6) — the budget gate above plus
                // verification decide.
                run.transition_to(RecoveryRunStatus::Diagnosing).ok();
                let diagnosis = self.diagnose(id, self.clock.now_unix_ms());
                self.store
                    .save_diagnosis(Some(&run.id), &diagnosis, self.clock.now_unix_ms());
                run.diagnosis = Some(diagnosis);
                self.store.save_run(&run);
            }

            match plan.failure_handling {
                custos_model::FailureHandling::TryNextPlan => continue,
                custos_model::FailureHandling::Escalate | custos_model::FailureHandling::Stop => {
                    self.finish_escalated(&mut run, id);
                    self.apply_cooldown(id);
                    return run;
                }
            }
        }

        // Every applicable plan failed: stop automatic handling (§6.3).
        self.finish_escalated(&mut run, id);
        self.apply_cooldown(id);
        run
    }

    /// Budget gate before every attempt: run duration, restart rate and
    /// high-risk count (docs/guide/recovery-engine.md §6.5).
    fn budget_gate(
        &mut self,
        id: &ResourceId,
        run: &RecoveryRun,
        plan: &RecoveryPlan,
    ) -> Result<(), BudgetExceeded> {
        let now = self.clock.now_unix_ms();
        if self.budget_state.attempts_for(id) >= self.budget.max_attempts_per_resource {
            return Err(BudgetExceeded::AttemptsPerResource {
                resource_id: id.clone(),
                limit: self.budget.max_attempts_per_resource,
            });
        }
        self.budget_state
            .check_run_duration(run.started_at_unix_ms, now, &self.budget)?;
        self.budget_state
            .check_restart_rate(id, &self.budget, now)?;
        if plan.risk_level == custos_model::RiskLevel::High {
            self.budget_state.check_high_risk(&self.budget)?;
        }
        Ok(())
    }

    /// Persists the attempt as `Executing` BEFORE the external action runs
    /// (§7.3), executes it, then records the outcome (§6.6 five states).
    fn execute_attempt(
        &mut self,
        id: &ResourceId,
        plan: &RecoveryPlan,
        attempt_no: u32,
        run: &mut RecoveryRun,
    ) -> ActionOutcome {
        let now = self.clock.now_unix_ms();
        let action_id = ActionId::new(self.next_id("action"));
        let mut attempt = ActionAttempt {
            action_id,
            plan_id: plan.plan_id.clone(),
            attempt_no,
            outcome: None,
            started_at_unix_ms: now,
            finished_at_unix_ms: None,
        };
        self.store.save_attempt(&run.id, &attempt);
        run.action_attempts.push(attempt.clone());
        // §6.1 path: Planning -> ResolvingDependencies -> Executing. Both hops
        // are no-ops when the run already sits in the later state.
        run.transition_to(RecoveryRunStatus::ResolvingDependencies)
            .ok();
        run.transition_to(RecoveryRunStatus::Executing).ok();
        self.store.emit(EngineEvent::ActionStarted {
            run_id: run.id.clone(),
            resource_id: id.clone(),
            plan_id: plan.plan_id.clone(),
            attempt_no,
            at_unix_ms: now,
        });

        let outcome = self
            .executor
            .execute(&plan.action, Duration::from_millis(plan.timeout_ms));
        let now = self.clock.now_unix_ms();
        attempt.outcome = Some(outcome);
        attempt.finished_at_unix_ms = Some(now);
        self.store.save_attempt(&run.id, &attempt);
        self.budget_state.record_attempt(id, now);
        if plan.risk_level == custos_model::RiskLevel::High {
            self.budget_state.record_high_risk();
        }
        self.store.emit(EngineEvent::ActionFinished {
            run_id: run.id.clone(),
            resource_id: id.clone(),
            plan_id: plan.plan_id.clone(),
            attempt_no,
            outcome,
            at_unix_ms: now,
        });
        tracing::info!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = id.as_str(),
            plan = plan.plan_id.as_str(),
            attempt = attempt_no,
            outcome = ?outcome,
            "action attempt finished"
        );
        outcome
    }

    fn finish_run(&mut self, run: &mut RecoveryRun, event: EngineEvent) {
        self.store.save_run(run);
        self.store.emit(event);
    }

    /// Closes the incident of a verified-recovered run; escalated runs keep
    /// their incident open for the human (§6.3).
    fn close_incident_of(&mut self, run: &RecoveryRun) {
        if let Some(incident_id) = &run.incident_id {
            let mut incident = custos_model::Incident::open(
                incident_id.clone(),
                run.resource_id.clone(),
                run.diagnosis
                    .as_ref()
                    .map(|d| d.fault_code)
                    .unwrap_or(FaultCode::UnknownFailure),
                run.diagnosis
                    .as_ref()
                    .map(|d| d.summary.clone())
                    .unwrap_or_default(),
                run.started_at_unix_ms,
            );
            incident.close(self.clock.now_unix_ms());
            self.store.save_incident(&incident);
        }
    }

    fn finish_escalated(&mut self, run: &mut RecoveryRun, id: &ResourceId) {
        run.transition_to(RecoveryRunStatus::Escalated).ok();
        self.store.save_step(
            &run.id,
            (run.action_attempts.len() + run.verification_results.len() + 1) as u32,
            "escalated",
            "automatic handling stopped",
            self.clock.now_unix_ms(),
        );
        tracing::warn!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = id.as_str(),
            attempts = run.action_attempts.len(),
            "recovery escalated: automatic handling stopped"
        );
        let event = EngineEvent::RecoveryExhausted {
            run_id: run.id.clone(),
            resource_id: id.clone(),
            at_unix_ms: self.clock.now_unix_ms(),
        };
        self.finish_run(run, event);
    }

    fn apply_cooldown(&mut self, id: &ResourceId) {
        let now = self.clock.now_unix_ms();
        let cooldown = self.managed[id]
            .resource
            .recovery_policy
            .plans
            .iter()
            .map(|p| p.cooldown_ms)
            .max()
            .unwrap_or(30_000);
        if let Some(managed) = self.managed.get_mut(id) {
            managed.cooldown_until_ms = now + cooldown;
        }
    }

    /// Maps the freshest probe evidence to a fault code (docs §5.1: probes
    /// collect facts, diagnosis judges).
    fn diagnose(&mut self, id: &ResourceId, now_unix_ms: u64) -> Diagnosis {
        let probe_kind = self
            .probes
            .get(id)
            .map(|p| p.name().to_owned())
            .unwrap_or_default();
        let status = self.managed[id].snapshot.status;
        let fault = match probe_kind.as_str() {
            "process" => FaultCode::ProcessExited,
            "exec" => match status {
                HealthStatus::Unreachable => FaultCode::ProbeTimeout,
                _ => FaultCode::RemoteEndpointUnreachable,
            },
            _ => FaultCode::UnknownFailure,
        };
        Diagnosis {
            fault_code: fault,
            summary: format!("status {status:?} reported by {probe_kind} probe"),
            confidence: None,
            affected_resources: vec![id.clone()],
            evidence_refs: vec![format!("probe:{probe_kind}@{now_unix_ms}")],
        }
    }

    /// Verifies a recovery by re-probing until the success threshold
    /// confirms health (docs/guide/agent-brief.md rule 6).
    fn verify(&mut self, id: &ResourceId, plan: &RecoveryPlan, run: &mut RecoveryRun) -> bool {
        let now = self.clock.now_unix_ms();
        let success_threshold = self.managed[id]
            .resource
            .health_policy
            .success_threshold
            .max(1);
        let mut passed = true;
        for _ in 0..success_threshold {
            let report = self.probe_one(id, now);
            if !report.succeeded {
                passed = false;
            }
        }
        let result = VerificationResult {
            resource_id: id.clone(),
            passed,
            summary: format!("verification condition of plan `{}`", plan.plan_id),
            evidence_refs: vec![format!("probe:after:{}", plan.plan_id)],
            checked_at_unix_ms: self.clock.now_unix_ms(),
        };
        self.store.save_verification(&result);
        run.verification_results.push(result.clone());
        self.store.emit(EngineEvent::VerificationRecorded {
            run_id: run.id.clone(),
            resource_id: id.clone(),
            passed,
            at_unix_ms: result.checked_at_unix_ms,
        });
        tracing::debug!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = id.as_str(),
            plan = plan.plan_id.as_str(),
            passed,
            "verification checked"
        );
        passed
    }

    /// §3.6 dependency resolution: re-probe binding dependencies, recover
    /// unhealthy ones (recursively), then re-verify — never assume success.
    ///
    /// Returns false when the run must stop driving. That is either an
    /// escalation or a suspension: when the dependency is already being
    /// recovered (or is cooling down towards its next attempt), the run
    /// waits on that SAME result instead of escalating alongside it — all
    /// dependents of one dependency share a single recovery (dedup).
    fn dependencies_satisfied(
        &mut self,
        id: &ResourceId,
        run: &mut RecoveryRun,
        step: &mut u32,
    ) -> bool {
        let dependencies = self.managed[id].resource.dependencies.clone();
        for dep in &dependencies {
            if !dep.is_binding(&self.active_modes) {
                continue;
            }
            let target = dep.target_resource_id.clone();
            // Re-probe: state must be fresh, not assumed (§2.3 rules 4/5).
            self.probe_one(&target, self.clock.now_unix_ms());
            if self.managed[&target].snapshot.status != HealthStatus::Healthy {
                // Recover the dependency first (its own policy and budgets),
                // unless a recovery is already in flight or its next attempt
                // is scheduled behind a cooldown — then wait on that result.
                if self.active_runs.contains(&target)
                    || now_lt(
                        self.clock.now_unix_ms(),
                        self.managed[&target].cooldown_until_ms,
                    )
                {
                    self.suspend_on(run, &target, step);
                    return false;
                }
                let _ = self.try_recover(&target);
                // Re-verify after dependency recovery.
                self.probe_one(&target, self.clock.now_unix_ms());
                if self.managed[&target].snapshot.status != HealthStatus::Healthy {
                    if now_lt(
                        self.clock.now_unix_ms(),
                        self.managed[&target].cooldown_until_ms,
                    ) {
                        // The dependency just failed an attempt and is cooling
                        // down towards its next one: wait for that result.
                        self.suspend_on(run, &target, step);
                        return false;
                    }
                    self.store.emit(EngineEvent::DependencyUnsatisfied {
                        run_id: run.id.clone(),
                        resource_id: id.clone(),
                        dependency_id: target.clone(),
                        at_unix_ms: self.clock.now_unix_ms(),
                    });
                    self.store.save_step(
                        &run.id,
                        bump(step),
                        "resolving_dependencies",
                        &format!("dependency `{}` unsatisfied", target.as_str()),
                        self.clock.now_unix_ms(),
                    );
                    tracing::warn!(
                        target: "custos::engine",
                        run = run.id.as_str(),
                        resource = id.as_str(),
                        dependency = target.as_str(),
                        "dependency unsatisfied after recovery: upstream action blocked"
                    );
                    self.finish_escalated(run, id);
                    return false;
                }
            }
        }
        true
    }

    /// Suspends the run in `Waiting` on `target`, registering it as a
    /// waiter (§3.6 dedup). On wait-budget exhaustion the run escalates
    /// instead. Either way driving stops for this tick.
    fn suspend_on(&mut self, run: &mut RecoveryRun, target: &ResourceId, step: &mut u32) {
        let cycles = self.wait_cycles.entry(run.id.clone()).or_insert(0);
        *cycles += 1;
        if *cycles > MAX_WAIT_CYCLES {
            self.wait_cycles.remove(&run.id);
            tracing::warn!(
                target: "custos::engine",
                run = run.id.as_str(),
                resource = run.resource_id.as_str(),
                dependency = target.as_str(),
                cycles = MAX_WAIT_CYCLES,
                "wait budget exhausted: escalating"
            );
            let id = run.resource_id.clone();
            self.finish_escalated(run, &id);
            return;
        }
        let until = self.managed[target].cooldown_until_ms;
        let reason = format!("awaiting dependency `{}` recovery", target.as_str());
        let run_id = run.id.clone();
        if run.suspend(reason, Some(until)).is_err() {
            let id = run.resource_id.clone();
            self.finish_escalated(run, &id);
            return;
        }
        self.store.save_step(
            &run_id,
            bump(step),
            "waiting",
            &format!("dependency `{}` recovery in flight", target.as_str()),
            self.clock.now_unix_ms(),
        );
        self.waiters
            .entry(target.clone())
            .or_default()
            .push(run.clone());
        tracing::info!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = run.resource_id.as_str(),
            dependency = target.as_str(),
            wait_until_unix_ms = until,
            "run waiting on dependency recovery"
        );
        self.store.save_run(run);
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
            if changed || !run.status.is_terminal() {
                run.status = RecoveryRunStatus::OutcomeUnknown;
                run.updated_at_unix_ms = now;
                self.store.save_run(&run);
                self.store.emit(EngineEvent::OutcomeUnknown {
                    run_id: run.id.clone(),
                    resource_id: resource_id.clone(),
                    at_unix_ms: now,
                });
                tracing::warn!(
                    target: "custos::engine",
                    run = run.id.as_str(),
                    resource = resource_id.as_str(),
                    "resumed unfinished run: outcome may be unknown, not re-executing"
                );
                affected += 1;
            }
        }
        affected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ScriptedExecutor;
    use custos_model::{
        ActionSpec, Dependency, DependencyGraph, FailureHandling, HealthCondition, HealthPolicy,
        RiskLevel,
    };

    /// Test probe with a scripted sequence of outcomes; exhausted = failing.
    struct ScriptedProbe {
        results: std::sync::Mutex<std::collections::VecDeque<bool>>,
    }

    impl ScriptedProbe {
        fn new(results: Vec<bool>) -> Self {
            Self {
                results: std::sync::Mutex::new(results.into()),
            }
        }
    }

    impl Probe for ScriptedProbe {
        fn name(&self) -> &'static str {
            // "process" makes the engine diagnose ProcessExited.
            "process"
        }

        fn probe(&self, resource: &Resource) -> ProbeReport {
            let succeeded = self
                .results
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .unwrap_or(false);
            ProbeReport {
                resource_id: resource.id.clone(),
                probe_name: self.name().to_owned(),
                succeeded,
                status: if succeeded {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                },
                observation: String::new(),
            }
        }
    }

    fn plan(id: &str, attempts: u32, risk: RiskLevel) -> RecoveryPlan {
        RecoveryPlan {
            plan_id: id.to_owned(),
            applicable_faults: vec![FaultCode::ProcessExited],
            preconditions: vec![],
            risk_level: risk,
            action: ActionSpec {
                program: "true".to_owned(),
                args: vec![],
            },
            timeout_ms: 5_000,
            max_attempts: attempts,
            cooldown_ms: 1_000,
            idempotent: true,
            verification: HealthCondition::StatusIs(HealthStatus::Healthy),
            failure_handling: FailureHandling::TryNextPlan,
            compensation: None,
        }
    }

    fn resource(id: &str, plans: Vec<RecoveryPlan>, deps: Vec<Dependency>) -> Resource {
        let mut r = Resource::new(id, "process");
        r.health_policy = HealthPolicy {
            failure_threshold: 1,
            success_threshold: 1,
        };
        r.recovery_policy.plans = plans;
        r.dependencies = deps;
        r
    }

    fn test_engine(
        resources: Vec<Resource>,
        edges: Vec<(ResourceId, Vec<ResourceId>)>,
        probe_results: Vec<Vec<bool>>,
        executor_steps: Vec<ActionOutcome>,
    ) -> (Engine, Arc<crate::MemoryStore>, Arc<ScriptedExecutor>) {
        let (engine, store, scripted, _clock) =
            test_engine_with_clock(resources, edges, probe_results, executor_steps);
        (engine, store, scripted)
    }

    fn test_engine_with_clock(
        resources: Vec<Resource>,
        edges: Vec<(ResourceId, Vec<ResourceId>)>,
        probe_results: Vec<Vec<bool>>,
        executor_steps: Vec<ActionOutcome>,
    ) -> (
        Engine,
        Arc<crate::MemoryStore>,
        Arc<ScriptedExecutor>,
        Arc<crate::clock::FakeClock>,
    ) {
        let store = Arc::new(crate::MemoryStore::default());
        // Shared handle so tests can assert which actions actually ran.
        let scripted = std::sync::Arc::new(ScriptedExecutor::new(executor_steps));
        let executor: Box<dyn ActionExecutor> = {
            let scripted = scripted.clone();
            Box::new(
                move |action: &custos_model::ActionSpec, timeout: Duration| {
                    scripted.execute(action, timeout)
                },
            )
        };
        let mut probes = HashMap::new();
        for (resource, results) in resources.iter().zip(probe_results) {
            probes.insert(
                resource.id.clone(),
                Box::new(ScriptedProbe::new(results)) as Box<dyn Probe>,
            );
        }
        let config = ValidatedConfig {
            resources,
            alternative_groups: vec![],
            graph: DependencyGraph::build(edges).expect("test graph"),
            permissions: Default::default(),
            notification: Default::default(),
        };
        let clock = Arc::new(crate::clock::FakeClock::new(1_000));
        let engine = Engine::new(&config, probes, executor, store.clone(), clock.clone());
        (engine, store, scripted, clock)
    }

    fn required_dep(target: &str) -> Dependency {
        Dependency::required(ResourceId::new(target))
    }

    /// Two dependents park on the SAME dependency recovery (dedup): the
    /// dependency is retried once per opportunity, both dependents resume
    /// through `Waiting -> Planning` when it turns healthy.
    #[test]
    fn dependents_wait_on_one_dependency_recovery_and_resume() {
        let (mut engine, store, exec, clock) = test_engine_with_clock(
            vec![
                resource("db", vec![plan("restart-db", 1, RiskLevel::Low)], vec![]),
                resource(
                    "app1",
                    vec![plan("restart-app1", 1, RiskLevel::Low)],
                    vec![required_dep("db")],
                ),
                resource(
                    "app2",
                    vec![plan("restart-app2", 1, RiskLevel::Low)],
                    vec![required_dep("db")],
                ),
            ],
            vec![
                (ResourceId::new("db"), vec![]),
                (ResourceId::new("app1"), vec![ResourceId::new("db")]),
                (ResourceId::new("app2"), vec![ResourceId::new("db")]),
            ],
            // db: observe fail, verify fail (action 1 fails), two dep checks
            // by app1/app2, tick1-end wake observe fail, tick2 wake observe
            // fail, retry-verify true, re-probe true, dep checks by both
            // redriven apps.
            vec![
                vec![
                    false, false, false, false, false, false, true, true, true, true,
                ],
                // app1: observe fail, verify true (after its own action).
                vec![false, true],
                // app2: observe fail, verify true.
                vec![false, true],
            ],
            // db action fails first, succeeds on the retry; apps succeed.
            vec![
                ActionOutcome::Failed,
                ActionOutcome::Succeeded,
                ActionOutcome::Succeeded,
                ActionOutcome::Succeeded,
            ],
        );

        // Tick 1: db's own recovery fails and cools down; both apps see the
        // unsatisfied dependency cooling down and suspend on the SAME result.
        let runs = engine.tick();
        assert_eq!(runs.len(), 3, "db escalated + two apps waiting");
        let db_run = runs
            .iter()
            .find(|r| r.resource_id.as_str() == "db")
            .unwrap();
        assert_eq!(db_run.status, RecoveryRunStatus::Escalated);
        for app in ["app1", "app2"] {
            let run = runs.iter().find(|r| r.resource_id.as_str() == app).unwrap();
            assert_eq!(run.status, RecoveryRunStatus::Waiting);
            assert_eq!(
                run.wait_reason.as_deref(),
                Some("awaiting dependency `db` recovery")
            );
            assert!(run.action_attempts.is_empty(), "no upstream action ran");
        }

        // Cooldown (1s) expires; the wake pass retries db once and both apps
        // resume on the shared result.
        clock.advance_ms(1_500);
        let resumed = engine.tick();
        assert_eq!(resumed.len(), 2, "both apps drove to completion");
        for run in &resumed {
            assert_eq!(run.status, RecoveryRunStatus::Recovered);
            assert_eq!(run.action_attempts.len(), 1);
        }

        // Dedup: db was recovered exactly twice in total (initial failure +
        // one retry) — never once per dependent.
        assert_eq!(exec.remaining(), 0, "every scripted action consumed");
        let db_attempts: usize = store
            .steps()
            .iter()
            .filter(|s| s.phase == "executing" && s.detail.contains("restart-db"))
            .count();
        assert_eq!(
            db_attempts, 2,
            "db executed once per attempt, not per dependent"
        );

        // Persistence: waiting steps and closed incidents are recorded.
        let phases: Vec<String> = store.steps().into_iter().map(|s| s.phase).collect();
        assert!(
            phases.contains(&"waiting".to_owned()),
            "waiting steps recorded: {phases:?}"
        );
        assert!(phases.contains(&"resolving_dependencies".to_owned()));
        let incidents = store.incidents();
        assert_eq!(incidents.len(), 4, "one incident per run");
        assert_eq!(
            incidents.iter().filter(|i| i.is_open()).count(),
            1,
            "only db's escalated incident stays open"
        );
    }

    /// A `Waiting` run restored from the store after a crash is re-driven,
    /// never marked outcome-unknown (§6.6 applies to executed actions only).
    #[test]
    fn waiting_run_from_store_resumes_instead_of_unknown() {
        let (mut engine, store, _exec, _clock) = test_engine_with_clock(
            vec![resource(
                "svc",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            // The resource is healthy again by the time the engine observes.
            vec![vec![true]],
            vec![],
        );

        // A previous process died with the run parked in Waiting.
        let mut seeded = RecoveryRun::new(
            RecoveryRunId::new("run-seeded"),
            ResourceId::new("svc"),
            "failure threshold reached",
            900,
        );
        seeded
            .transition_to(RecoveryRunStatus::ProblemDetected)
            .ok();
        seeded.transition_to(RecoveryRunStatus::Diagnosing).ok();
        seeded.transition_to(RecoveryRunStatus::Planning).ok();
        seeded
            .suspend("awaiting dependency `db` recovery", None)
            .expect("suspend from planning");
        seeded.diagnosis = Some(Diagnosis {
            fault_code: FaultCode::ProcessExited,
            summary: "seeded".into(),
            confidence: None,
            affected_resources: vec![ResourceId::new("svc")],
            evidence_refs: vec![],
        });
        store.save_run(&seeded);

        assert_eq!(engine.resume_unfinished(), 0, "waiting is not unknown");
        let runs = engine.tick();
        let seeded_run = store.run(&RecoveryRunId::new("run-seeded")).unwrap();
        assert_ne!(
            seeded_run.status,
            RecoveryRunStatus::OutcomeUnknown,
            "waiting runs never become outcome-unknown"
        );
        assert_eq!(seeded_run.status, RecoveryRunStatus::Cancelled);
        assert!(runs.is_empty(), "healthy resource needs no new run");
    }

    /// Fault injection §16.3 case 5: the dependency re-faults while the
    /// dependent is mid-recovery; the next attempt must NOT run upstream.
    #[test]
    fn dependency_refault_between_attempts_blocks_next_action() {
        let (mut engine, store, exec, _clock) = test_engine_with_clock(
            vec![
                // db has no recovery plan: once it re-faults it stays broken.
                resource("db", vec![], vec![]),
                resource(
                    "app",
                    vec![plan("restart", 2, RiskLevel::Low)],
                    vec![required_dep("db")],
                ),
            ],
            vec![
                (ResourceId::new("db"), vec![]),
                (ResourceId::new("app"), vec![ResourceId::new("db")]),
            ],
            // db: own observe healthy; dep-check before attempt 1 healthy;
            // before attempt 2 unhealthy; still unhealthy after the
            // (stopped) recovery attempt.
            vec![vec![true, true, false, false], vec![false, false]],
            // Only attempt 1 of the app may execute.
            vec![ActionOutcome::Succeeded],
        );
        let runs = engine.tick();
        let app_run = runs
            .iter()
            .find(|r| r.resource_id.as_str() == "app")
            .expect("app run");
        assert_eq!(app_run.status, RecoveryRunStatus::Escalated);
        assert_eq!(
            app_run.action_attempts.len(),
            1,
            "attempt 2 must be blocked by the re-faulted dependency"
        );
        assert_eq!(exec.remaining(), 0, "no second action executed");
        assert!(store.event_types().contains(&"dependency.unsatisfied"));
    }

    /// Fault injection §16.3 case 8: every store write fails. The engine
    /// degrades to running without persistence — recovery still completes
    /// and nothing panics (a broken observability path must not break the
    /// healing path).
    #[test]
    fn failing_store_degrades_without_stalling_recovery() {
        struct WriteFailingStore;
        impl crate::events::RunStore for WriteFailingStore {
            fn save_run(&self, _run: &RecoveryRun) {}
            fn save_attempt(
                &self,
                _run_id: &RecoveryRunId,
                _attempt: &custos_model::ActionAttempt,
            ) {
            }
            fn save_verification(&self, _result: &VerificationResult) {}
            fn emit(&self, _event: EngineEvent) {}
            fn unfinished_runs(&self) -> Vec<RecoveryRun> {
                Vec::new()
            }
        }

        let store = Arc::new(WriteFailingStore);
        let scripted = Arc::new(ScriptedExecutor::new(vec![ActionOutcome::Succeeded]));
        let executor: Box<dyn ActionExecutor> = {
            let scripted = scripted.clone();
            Box::new(
                move |action: &custos_model::ActionSpec, timeout: Duration| {
                    scripted.execute(action, timeout)
                },
            )
        };
        let svc = resource("svc", vec![plan("restart", 1, RiskLevel::Low)], vec![]);
        let mut probes = HashMap::new();
        probes.insert(
            svc.id.clone(),
            Box::new(ScriptedProbe::new(vec![false, true])) as Box<dyn Probe>,
        );
        let config = ValidatedConfig {
            resources: vec![svc.clone()],
            alternative_groups: vec![],
            graph: DependencyGraph::build(vec![(svc.id.clone(), vec![])]).expect("graph"),
            permissions: Default::default(),
            notification: Default::default(),
        };
        let mut engine = Engine::new(
            &config,
            probes,
            executor,
            store,
            Arc::new(crate::clock::FakeClock::new(1_000)),
        );
        // Must not panic despite every persistence call failing.
        let runs = engine.tick();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RecoveryRunStatus::Recovered);
        assert_eq!(runs[0].action_attempts.len(), 1);
    }

    #[test]
    fn recovers_when_probe_confirms_after_action() {
        // One failing probe trips the threshold; the action succeeds and the
        // verification probe confirms health → Recovered.
        let (mut engine, store, _exec) = test_engine(
            vec![resource(
                "svc",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![false, true]],
            vec![ActionOutcome::Succeeded],
        );
        let runs = engine.tick();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RecoveryRunStatus::Recovered);
        let types = store.event_types();
        assert!(types.contains(&"recovery.started"));
        assert!(types.contains(&"recovery.action_started"));
        assert!(types.contains(&"recovery.action_finished"));
        assert!(types.contains(&"recovery.verification"));
        assert!(types.contains(&"recovery.succeeded"));
    }

    #[test]
    fn escalates_when_verification_rejects_exit_success() {
        // Action exit 0 but the probe still says unhealthy: verification must
        // win (docs/guide/agent-brief.md rule 6), so the run escalates.
        let (mut engine, store, _) = test_engine(
            vec![resource(
                "svc",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![false, false]],
            vec![ActionOutcome::Succeeded],
        );
        let runs = engine.tick();
        assert_eq!(runs[0].status, RecoveryRunStatus::Escalated);
        assert!(store.event_types().contains(&"recovery.exhausted"));
    }

    #[test]
    fn second_plan_runs_after_first_fails() {
        let (mut engine, _store, _) = test_engine(
            vec![resource(
                "svc",
                vec![plan("p1", 1, RiskLevel::Low), plan("p2", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            // tick probe fails, p1 verification fails, p2 verification passes.
            vec![vec![false, false, true]],
            vec![ActionOutcome::Failed, ActionOutcome::Succeeded],
        );
        let runs = engine.tick();
        assert_eq!(runs[0].status, RecoveryRunStatus::Recovered);
        assert_eq!(runs[0].action_attempts.len(), 2);
        assert_eq!(runs[0].action_attempts[0].plan_id, "p1");
        assert_eq!(runs[0].action_attempts[1].plan_id, "p2");
    }

    #[test]
    fn unsatisfied_dependency_blocks_upstream_action() {
        // app requires db; db is unhealthy and has no plans. The engine must
        // not run app's action (agent-brief rule 5) and must escalate.
        let app = resource(
            "app",
            vec![plan("restart-app", 1, RiskLevel::Low)],
            vec![Dependency::required("db")],
        );
        let db = resource("db", vec![], vec![]);
        let (mut engine, store, exec) = test_engine(
            vec![db, app],
            vec![
                (ResourceId::new("app"), vec![ResourceId::new("db")]),
                (ResourceId::new("db"), vec![]),
            ],
            vec![vec![false, false], vec![false]],
            vec![ActionOutcome::Succeeded],
        );
        let runs = engine
            .tick()
            .into_iter()
            .filter(|r| r.resource_id == ResourceId::new("app"))
            .collect::<Vec<_>>();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RecoveryRunStatus::Escalated);
        assert!(runs[0].action_attempts.is_empty());
        assert!(store.event_types().contains(&"dependency.unsatisfied"));
        // The scripted action was never consumed.
        assert_eq!(exec.remaining(), 1);
    }

    #[test]
    fn attempts_budget_trips_before_second_attempt() {
        let (mut engine, _store, _exec) = test_engine(
            vec![resource(
                "svc",
                vec![plan("restart", 3, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![false, false, false, false]],
            vec![ActionOutcome::Failed, ActionOutcome::Failed],
        );
        engine.set_budget(RecoveryBudget {
            max_attempts_per_resource: 1,
            ..RecoveryBudget::default()
        });
        let runs = engine.tick();
        assert_eq!(runs[0].status, RecoveryRunStatus::Escalated);
        // Plan allows 3 attempts; the budget stops it after the first.
        assert_eq!(runs[0].action_attempts.len(), 1);
    }

    #[test]
    fn busy_mutex_rejects_second_run() {
        let (mut engine, _store, _) = test_engine(
            vec![resource(
                "svc",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![false]],
            vec![],
        );
        let id = ResourceId::new("svc");
        // Drive the snapshot unhealthy first, as a tick would.
        engine.probe_one(&id, 1_000);
        engine.active_runs.insert(id.clone());
        assert!(matches!(
            engine.try_recover(&id),
            Err(RecoverError::ResourceBusy(_))
        ));
    }

    #[test]
    fn cooldown_suppresses_immediate_retry() {
        let (mut engine, _store, _) = test_engine(
            vec![resource(
                "svc",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![false, false]],
            vec![ActionOutcome::Failed],
        );
        let runs = engine.tick();
        assert_eq!(runs.len(), 1);
        // Unhealthy again on the next tick but inside cooldown → no new run.
        let runs2 = engine.tick();
        assert!(runs2.is_empty());
    }

    #[test]
    fn resume_unfinished_marks_outcome_unknown() {
        let (mut engine, store, _) = test_engine(
            vec![resource(
                "svc",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![]],
            vec![],
        );
        // Simulate a crash mid-execution: attempt persisted, outcome missing.
        let run_id = RecoveryRunId::new("run-crash");
        let mut run = RecoveryRun::new(
            run_id.clone(),
            ResourceId::new("svc"),
            "failure threshold reached",
            1_000,
        );
        run.status = RecoveryRunStatus::Executing;
        run.action_attempts.push(ActionAttempt {
            action_id: ActionId::new("action-1"),
            plan_id: "restart".to_owned(),
            attempt_no: 1,
            outcome: None,
            started_at_unix_ms: 1_000,
            finished_at_unix_ms: None,
        });
        store.save_run(&run);

        let affected = engine.resume_unfinished();
        assert_eq!(affected, 1);
        let resumed = store.run(&run_id).expect("run persisted");
        assert_eq!(resumed.status, RecoveryRunStatus::OutcomeUnknown);
        assert_eq!(
            resumed.action_attempts[0].outcome,
            Some(ActionOutcome::OutcomeUnknown)
        );
        assert!(store.event_types().contains(&"recovery.outcome_unknown"));
    }
}

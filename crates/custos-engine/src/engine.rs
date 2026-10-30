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
    seq: u64,
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
        let mut started = Vec::new();
        for id in self.probe_order.clone() {
            let interval_ms = self.managed[&id].interval_ms;
            if !self.tracker.is_due(&id, interval_ms, now) {
                continue;
            }
            self.probe_one(&id, now);
            if let Ok(run) = self.try_recover(&id) {
                started.push(run);
            }
        }
        started
    }

    /// Probes one resource and applies jitter-suppressed threshold logic.
    pub fn probe_one(&mut self, id: &ResourceId, now_unix_ms: u64) -> ProbeReport {
        let (report, thresholds) = {
            let managed = &self.managed[id];
            let probe = self
                .probes
                .get(id)
                .expect("probe wired for every resource at construction");
            (
                probe.probe(&managed.resource),
                (
                    managed.resource.health_policy.failure_threshold,
                    managed.resource.health_policy.success_threshold,
                ),
            )
        };
        let snapshot = &mut self.managed.get_mut(id).unwrap().snapshot;
        snapshot.record_probe(report.succeeded, thresholds.0, thresholds.1, now_unix_ms);
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

    /// The full recovery cycle for one resource.
    fn recover(&mut self, id: &ResourceId) -> RecoveryRun {
        let now = self.clock.now_unix_ms();
        let run_id = RecoveryRunId::new(self.next_id("run"));
        let incident = IncidentId::new(self.next_id("inc"));
        let mut run = RecoveryRun::new(run_id, id.clone(), "failure threshold reached", now);
        run.incident_id = Some(incident);
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
            run.diagnosis = Some(diagnosis);
            fault
        };
        self.store.save_run(&run);

        // Resolve dependencies before touching the resource (§3.6).
        run.transition_to(RecoveryRunStatus::Planning).ok();
        run.transition_to(RecoveryRunStatus::ResolvingDependencies)
            .ok();
        self.store.save_run(&run);
        if !self.dependencies_satisfied(id, &mut run) {
            // §15 rule 5: upstream actions must not run on unsatisfied deps.
            self.finish_escalated(&mut run, id);
            return run;
        }

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
                // execution (§6.1); execute_attempt adds the
                // ResolvingDependencies hop into Executing.
                run.transition_to(RecoveryRunStatus::Planning).ok();
                if self.budget_gate(id, &run, plan).is_err() {
                    self.finish_escalated(&mut run, id);
                    self.apply_cooldown(id);
                    return run;
                }
                self.execute_attempt(id, plan, attempt_no, &mut run);
                run.transition_to(RecoveryRunStatus::Verifying).ok();
                if self.verify(id, plan, &mut run) {
                    run.transition_to(RecoveryRunStatus::Recovered).ok();
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
                    self.apply_cooldown(id);
                    return run;
                }
                // Fault remains or changed: re-diagnose before the next move
                // (docs/guide/recovery-engine.md §6.4). OutcomeUnknown never
                // re-executes blindly (§6.6) — the budget gate above plus
                // verification decide.
                run.transition_to(RecoveryRunStatus::Diagnosing).ok();
                let diagnosis = self.diagnose(id, self.clock.now_unix_ms());
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
        outcome
    }

    fn finish_run(&mut self, run: &mut RecoveryRun, event: EngineEvent) {
        self.store.save_run(run);
        self.store.emit(event);
    }

    fn finish_escalated(&mut self, run: &mut RecoveryRun, id: &ResourceId) {
        run.transition_to(RecoveryRunStatus::Escalated).ok();
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
        passed
    }

    /// §3.6 dependency resolution: re-probe binding dependencies, recover
    /// unhealthy ones (recursively), then re-verify — never assume success.
    fn dependencies_satisfied(&mut self, id: &ResourceId, run: &mut RecoveryRun) -> bool {
        let dependencies = self.managed[id].resource.dependencies.clone();
        for dep in &dependencies {
            if !dep.is_binding(&self.active_modes) {
                continue;
            }
            let target = &dep.target_resource_id;
            // Re-probe: state must be fresh, not assumed (§2.3 rules 4/5).
            self.probe_one(target, self.clock.now_unix_ms());
            if self.managed[target].snapshot.status != HealthStatus::Healthy {
                // Recover the dependency first (its own policy and budgets).
                let _ = self.try_recover(target);
                // Re-verify after dependency recovery.
                self.probe_one(target, self.clock.now_unix_ms());
                if self.managed[target].snapshot.status != HealthStatus::Healthy {
                    self.store.emit(EngineEvent::DependencyUnsatisfied {
                        run_id: run.id.clone(),
                        resource_id: id.clone(),
                        dependency_id: target.clone(),
                        at_unix_ms: self.clock.now_unix_ms(),
                    });
                    return false;
                }
            }
        }
        true
    }

    /// Crash recovery (§6.6, §7.3): attempts persisted as `Executing` have
    /// unknown outcomes; mark them so, never blindly re-execute. Returns the
    /// number of affected runs.
    pub fn resume_unfinished(&mut self) -> usize {
        let unfinished = self.store.unfinished_runs();
        let mut affected = 0;
        for mut run in unfinished {
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
                    resource_id,
                    at_unix_ms: now,
                });
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
        };
        let engine = Engine::new(
            &config,
            probes,
            executor,
            store.clone(),
            Arc::new(crate::clock::FakeClock::new(1_000)),
        );
        (engine, store, scripted)
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

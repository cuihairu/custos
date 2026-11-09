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
use custos_model::AlternativeGroup;
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

/// The notification sink: receives structured `NotificationEvent`s produced
/// by health transitions and the recovery state machine. Implementations
/// (the notification engine) must enqueue without blocking — the engine
/// calls it inline on its tick path (§8.1).
pub type EventEmitter = std::sync::Arc<dyn Fn(&custos_model::NotificationEvent) + Send + Sync>;

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
    /// Optional notification sink (§8.1). `None` until the runtime wires
    /// the notification engine in.
    events: Option<EventEmitter>,
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
    /// N-of-M backup groups (§3.3 AlternativeGroup): satisfied by member
    /// count, never by "every member healthy".
    alternative_groups: Vec<AlternativeGroup>,
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

mod dependency;
mod diagnose;
mod observation;
mod orchestrate;
mod recover;

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
            events: None,
            active_modes: HashSet::new(),
            active_runs: HashSet::new(),
            waiters: HashMap::new(),
            wait_cycles: HashMap::new(),
            resume_queue: Vec::new(),
            alternative_groups: config.alternative_groups.clone(),
            seq: 0,
        }
    }
    pub fn set_budget(&mut self, budget: RecoveryBudget) {
        self.budget = budget;
    }
    /// Attaches the notification sink (docs/guide/notification.md §8.1: the
    /// core engine produces structured events, never calls channel APIs).
    /// The callback is expected to enqueue, never block; without one the
    /// engine simply records its audit trail as before.
    pub fn set_event_emitter(&mut self, emitter: EventEmitter) {
        self.events = Some(emitter);
    }
    /// Builds and forwards one structured notification event to the sink,
    /// if any. Fire-and-forget by contract.
    #[allow(clippy::too_many_arguments)]
    fn emit_notification(
        &mut self,
        event_type: custos_model::EventType,
        severity: custos_model::Severity,
        summary: String,
        resource: &ResourceId,
        run: Option<&RecoveryRunId>,
        incident: Option<&custos_model::IncidentId>,
    ) {
        let Some(emitter) = self.events.clone() else {
            return;
        };
        let now = self.clock.now_unix_ms();
        let mut event = custos_model::NotificationEvent::new(
            custos_model::EventId::new(self.next_id("evt")),
            event_type,
            severity,
            summary,
            now,
        )
        .with_resource_id(resource.clone());
        if let Some(run) = run {
            event = event.with_recovery_run_id(run.clone());
        }
        if let Some(incident) = incident {
            event = event.with_incident_id(incident.clone());
        }
        emitter(&event);
    }
    pub fn snapshot(&self, id: &ResourceId) -> Option<&ResourceHealthSnapshot> {
        self.managed.get(id).map(|m| &m.snapshot)
    }
    fn next_id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}-{}", self.seq)
    }
    /// Builds a one-line summary for a health-transition notification: the
    /// probe observation when present, else the bare transition label.
    fn transition_summary(label: &str, observation: &str) -> String {
        if observation.trim().is_empty() {
            label.to_owned()
        } else {
            format!("{label}: {observation}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ScriptedExecutor;
    use custos_model::{
        ActionSpec, Dependency, DependencyGraph, FailureHandling, HealthCondition, HealthPolicy,
        NotificationEvent, RiskLevel, Severity,
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
            dashboard: Default::default(),
        };
        let clock = Arc::new(crate::clock::FakeClock::new(1_000));
        let engine = Engine::new(&config, probes, executor, store.clone(), clock.clone());
        (engine, store, scripted, clock)
    }

    fn required_dep(target: &str) -> Dependency {
        Dependency::required(ResourceId::new(target))
    }

    fn group_dep(group: &str, anchor: &str) -> Dependency {
        Dependency {
            target_resource_id: ResourceId::new(anchor),
            requirement: custos_model::DependencyRequirement::AlternativeGroup,
            condition: HealthCondition::StatusIs(HealthStatus::Healthy),
            alternative_group_id: Some(group.to_owned()),
            conditional_on: None,
            timeout: None,
        }
    }

    fn test_engine_with_groups(
        resources: Vec<Resource>,
        edges: Vec<(ResourceId, Vec<ResourceId>)>,
        probe_results: Vec<Vec<bool>>,
        executor_steps: Vec<ActionOutcome>,
        alternative_groups: Vec<custos_model::AlternativeGroup>,
    ) -> (Engine, Arc<crate::MemoryStore>) {
        let (engine, store, _scripted, _clock) = {
            let store = Arc::new(crate::MemoryStore::default());
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
                alternative_groups,
                graph: DependencyGraph::build(edges).expect("test graph"),
                permissions: Default::default(),
                notification: Default::default(),
                dashboard: Default::default(),
            };
            let clock = Arc::new(crate::clock::FakeClock::new(1_000));
            let engine = Engine::new(&config, probes, executor, store.clone(), clock.clone());
            (engine, store, scripted, clock)
        };
        (engine, store)
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
            dashboard: Default::default(),
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

    /// A collector wired as the notification sink, for assertions.
    type EventCollector = std::sync::Arc<std::sync::Mutex<Vec<NotificationEvent>>>;

    fn collector() -> (EventCollector, EventEmitter) {
        let events: EventCollector = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = events.clone();
        let emitter: EventEmitter = std::sync::Arc::new(move |event: &NotificationEvent| {
            sink.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event.clone());
        });
        (events, emitter)
    }

    #[test]
    fn event_emitter_receives_fault_and_recovery_events() {
        // failure_threshold=1: one failed probe flips to Unhealthy, the plan
        // runs and verifies, so the whole fault lifecycle fires.
        let (engine, _store, _exec) = test_engine(
            vec![resource(
                "svc",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![false, true]],
            vec![ActionOutcome::Succeeded],
        );
        let mut engine = engine;
        let (events, emitter) = collector();
        engine.set_event_emitter(emitter);

        engine.tick();

        let received = events.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let kinds: Vec<&str> = received.iter().map(|e| e.event_type.as_str()).collect();
        assert!(kinds.contains(&"resource.unhealthy"), "{kinds:?}");
        assert!(kinds.contains(&"recovery.started"), "{kinds:?}");
        assert!(kinds.contains(&"recovery.action_started"), "{kinds:?}");
        assert!(kinds.contains(&"recovery.succeeded"), "{kinds:?}");
        // Every event is well-formed: id assigned, resource attached.
        for event in &received {
            assert!(!event.event_id.as_str().is_empty());
            assert_eq!(event.resource_id.as_ref().map(|r| r.as_str()), Some("svc"));
        }
    }

    /// A probe that always reports Unreachable with the plugin-degraded
    /// observation (the PluginProbe contract, §4.5 step 9).
    struct DegradedPluginProbe;

    impl Probe for DegradedPluginProbe {
        fn name(&self) -> &'static str {
            "plugin"
        }
        fn probe(&self, resource: &Resource) -> ProbeReport {
            ProbeReport {
                resource_id: resource.id.clone(),
                probe_name: self.name().to_owned(),
                succeeded: false,
                status: HealthStatus::Unreachable,
                observation: format!(
                    "{}: crashed 3 times within 30s",
                    custos_probe::PLUGIN_DEGRADED_OBSERVATION_PREFIX
                ),
            }
        }
    }

    #[test]
    fn unrecoverable_plugin_raises_plugin_unavailable() {
        let mut r = resource("pluginized", vec![], vec![]);
        r.resource_type = "plugin-backed".into();
        let (engine, _store, _exec) = test_engine(
            vec![r],
            vec![(ResourceId::new("pluginized"), vec![])],
            vec![],
            vec![],
        );
        let mut engine = engine;
        let (events, emitter) = collector();
        engine.set_event_emitter(emitter);
        engine
            .probes
            .insert(ResourceId::new("pluginized"), Box::new(DegradedPluginProbe));

        engine.tick();

        let received = events.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let plugin_faults: Vec<_> = received
            .iter()
            .filter(|e| e.event_type.as_str() == "plugin.unavailable")
            .collect();
        assert_eq!(
            plugin_faults.len(),
            1,
            "one fault on the transition, got {received:?}"
        );
        assert_eq!(plugin_faults[0].severity, Severity::Critical);
        assert!(plugin_faults[0]
            .summary
            .contains("plugin degraded (unprobeable)"));
        // The resource itself is marked Unreachable, never Unhealthy/Unusable.
        assert_eq!(
            engine
                .snapshot(&ResourceId::new("pluginized"))
                .map(|s| s.status),
            Some(HealthStatus::Unreachable)
        );
    }

    #[test]
    fn event_emitter_receives_exhausted_and_degraded_events() {
        // Both plans fail: the run escalates and notifies exhaustion.
        let (engine, _store, _exec) = test_engine(
            vec![resource(
                "svc",
                vec![plan("a", 1, RiskLevel::Low), plan("b", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("svc"), vec![])],
            vec![vec![false, false, false, false]],
            vec![ActionOutcome::Failed, ActionOutcome::Failed],
        );
        let mut engine = engine;
        let (events, emitter) = collector();
        engine.set_event_emitter(emitter);

        engine.tick();

        let received = events.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let kinds: Vec<&str> = received.iter().map(|e| e.event_type.as_str()).collect();
        assert!(kinds.contains(&"recovery.exhausted"), "{kinds:?}");
        assert!(kinds.contains(&"recovery.action_failed"), "{kinds:?}");
        // Severity defaults: exhaustion is critical, action failure warning.
        for event in &received {
            match event.event_type.as_str() {
                "recovery.exhausted" => assert_eq!(event.severity, Severity::Critical),
                "recovery.action_failed" => assert_eq!(event.severity, Severity::Warning),
                _ => {}
            }
        }
    }

    #[test]
    fn degraded_proxy_auth_does_not_trigger_recovery() {
        // A proxy returning 407 maps to Degraded: the process is alive and
        // credentials are the problem, so no recovery action may be started.
        struct AuthFailedProxyProbe;

        impl Probe for AuthFailedProxyProbe {
            fn name(&self) -> &'static str {
                "proxy"
            }

            fn probe(&self, resource: &Resource) -> ProbeReport {
                ProbeReport {
                    resource_id: resource.id.clone(),
                    probe_name: "proxy".into(),
                    succeeded: false,
                    status: HealthStatus::Degraded,
                    observation: "proxy is up but authentication failed (HTTP 407); \
                                 check credentials — not downtime"
                        .into(),
                }
            }
        }

        let (engine, store, _exec) = test_engine(
            vec![resource(
                "proxy",
                vec![plan("restart", 1, RiskLevel::Low)],
                vec![],
            )],
            vec![(ResourceId::new("proxy"), vec![])],
            vec![vec![false, false]],
            vec![ActionOutcome::Failed, ActionOutcome::Failed],
        );
        let mut engine = engine;
        engine
            .probes
            .insert(ResourceId::new("proxy"), Box::new(AuthFailedProxyProbe));

        engine.tick();

        assert_eq!(
            engine.snapshot(&ResourceId::new("proxy")).map(|s| s.status),
            Some(HealthStatus::Degraded)
        );
        assert!(
            store.events().is_empty(),
            "degraded must not open a recovery: {:?}",
            store.events()
        );
    }

    #[test]
    fn alternative_group_is_satisfied_by_member_count() {
        // N-of-M: one healthy member is enough for required=1, so `app` is
        // not blocked even though proxy-b is unhealthy.
        let group = custos_model::AlternativeGroup {
            group_id: "egress".into(),
            members: vec![ResourceId::new("proxy-a"), ResourceId::new("proxy-b")],
            required_count: 1,
        };
        let (mut engine, store) = test_engine_with_groups(
            vec![
                resource("proxy-a", vec![], vec![]),
                resource("proxy-b", vec![], vec![]),
                resource(
                    "app",
                    vec![plan("restart-app", 1, RiskLevel::Low)],
                    vec![group_dep("egress", "proxy-a")],
                ),
            ],
            vec![(ResourceId::new("app"), vec![])],
            // proxy-a healthy, proxy-b unhealthy, app unhealthy (needs recover).
            vec![
                vec![true, true, true, true, true, true, true, true],
                vec![false, false, false, false, false, false, false, false],
                vec![false, true],
            ],
            vec![ActionOutcome::Succeeded],
            vec![group],
        );
        // Trigger recovery of the app: the group (1-of-2) is satisfied by
        // proxy-a alone, so the app plan runs and verifies.
        let runs = engine.tick();
        let app_run = runs
            .iter()
            .find(|r| r.resource_id == ResourceId::new("app"))
            .expect("app recovery run");
        assert_eq!(app_run.status, RecoveryRunStatus::Recovered);
        assert_eq!(app_run.action_attempts.len(), 1);
        assert!(store.event_types().contains(&"recovery.succeeded"));
    }

    #[test]
    fn alternative_group_blocks_when_no_member_is_healthy() {
        // required=2 of 2 and both members unhealthy: the group is never
        // satisfied, so the app recovery escalates.
        let group = custos_model::AlternativeGroup {
            group_id: "egress".into(),
            members: vec![ResourceId::new("proxy-a"), ResourceId::new("proxy-b")],
            required_count: 2,
        };
        let (mut engine, _store) = test_engine_with_groups(
            vec![
                resource("proxy-a", vec![], vec![]),
                resource("proxy-b", vec![], vec![]),
                resource(
                    "app",
                    vec![plan("restart-app", 1, RiskLevel::Low)],
                    vec![group_dep("egress", "proxy-a")],
                ),
            ],
            vec![(ResourceId::new("app"), vec![])],
            vec![
                vec![false, false],
                vec![false, false],
                vec![false, false, true],
            ],
            vec![ActionOutcome::Succeeded],
            vec![group],
        );
        let runs = engine.tick();
        let app_run = runs
            .iter()
            .find(|r| r.resource_id == ResourceId::new("app"))
            .expect("app recovery run");
        assert_eq!(app_run.status, RecoveryRunStatus::Escalated);
        assert_eq!(app_run.action_attempts.len(), 0);
    }

    #[test]
    fn dependency_recovery_refusal_is_surfaced_in_escalation() {
        // C5: a refused dependency recovery (try_recover error) was silently
        // dropped — the run escalated as a bare "dependency unsatisfied"
        // with no trace of why no attempt happened. Here the dependency has
        // no recovery plans, so try_recover refuses; the refusal reason
        // must reach the persisted step record.
        let app = resource(
            "app",
            vec![plan("restart-app", 1, RiskLevel::Low)],
            vec![required_dep("dep")],
        );
        let dep = resource("dep", vec![], vec![]);
        let (mut engine, store, _scripted) = test_engine(
            vec![app, dep],
            vec![
                (ResourceId::new("dep"), vec![]),
                (ResourceId::new("app"), vec![ResourceId::new("dep")]),
            ],
            vec![vec![false], vec![false]],
            vec![],
        );
        engine.tick();
        let exhausted = store
            .events()
            .iter()
            .find_map(|e| match e {
                EngineEvent::RecoveryExhausted {
                    run_id,
                    resource_id,
                    ..
                } if resource_id.as_str() == "app" => Some(run_id.clone()),
                _ => None,
            })
            .expect("upstream run escalated");
        let detail = store
            .steps_of(&exhausted)
            .into_iter()
            .rev()
            .find(|s| s.phase == "resolving_dependencies")
            .expect("dependency step recorded")
            .detail;
        assert!(detail.contains("recovery refused"), "{detail}");
    }

    #[test]
    fn group_member_recovery_refusal_is_surfaced_in_escalation() {
        // C5 on the alternative-group path: members whose recovery is
        // refused (no plans) must be named with the refusal reason in the
        // persisted group-unsatisfied step, not silently skipped.
        let group = custos_model::AlternativeGroup {
            group_id: "egress".into(),
            members: vec![ResourceId::new("proxy-a"), ResourceId::new("proxy-b")],
            required_count: 2,
        };
        let (mut engine, store) = test_engine_with_groups(
            vec![
                resource("proxy-a", vec![], vec![]),
                resource("proxy-b", vec![], vec![]),
                resource(
                    "app",
                    vec![plan("restart-app", 1, RiskLevel::Low)],
                    vec![group_dep("egress", "proxy-a")],
                ),
            ],
            vec![(ResourceId::new("app"), vec![])],
            vec![vec![false], vec![false], vec![false]],
            vec![],
            vec![group],
        );
        engine.tick();
        let exhausted = store
            .events()
            .iter()
            .find_map(|e| match e {
                EngineEvent::RecoveryExhausted {
                    run_id,
                    resource_id,
                    ..
                } if resource_id.as_str() == "app" => Some(run_id.clone()),
                _ => None,
            })
            .expect("upstream run escalated");
        let detail = store
            .steps_of(&exhausted)
            .into_iter()
            .rev()
            .find(|s| s.phase == "resolving_dependencies")
            .expect("group step recorded")
            .detail;
        assert!(
            detail.contains("proxy-a") && detail.contains("recovery refused"),
            "{detail}"
        );
    }
}

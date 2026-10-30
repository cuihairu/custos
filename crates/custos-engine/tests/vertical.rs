//! Vertical integration tests: real config → builtin probes → engine →
//! SQLite store, with real processes (docs/guide/roadmap.md P0 acceptance).
//!
//! Scenarios: worker respawn end-to-end, dependency-ordered recovery
//! (database before its dependent app), and crash resume over SQLite.

use custos_config::load;
use custos_engine::{
    ActionExecutor, BuiltinExecExecutor, Engine, MemoryStore, RunStore, SystemClock,
};
use custos_model::ids::{ActionId, RecoveryRunId, ResourceId};
use custos_model::{ActionAttempt, ActionSpec, RecoveryRun, RecoveryRunStatus};
use custos_probe::Probe;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Spawn a detached `sleep` that writes its own pid to `pid_file`; returns
/// immediately. The engine's respawn action uses the same trick.
fn spawn_worker(pid_file: &str) {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "setsid sh -c 'echo $$ > {pid_file}; exec sleep 30' >/dev/null 2>&1 </dev/null &"
        ))
        .status()
        .expect("spawn worker");
    assert!(status.success(), "worker spawn failed");
}

fn read_pid(pid_file: &str) -> Option<u32> {
    std::fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

fn alive(pid: u32) -> bool {
    std::path::Path::new("/proc").join(pid.to_string()).exists()
}

fn build(config_toml: &str, db_path: Option<&std::path::Path>) -> (Engine, Arc<MemoryStore>) {
    let config = load(config_toml).expect("config");
    let mut probes: HashMap<ResourceId, Box<dyn Probe>> = HashMap::new();
    for resource in &config.resources {
        probes.insert(
            resource.id.clone(),
            custos_probe::factory::build_probe(resource).expect("builtin probe"),
        );
    }
    let store = Arc::new(MemoryStore::default());
    let engine = Engine::new(
        &config,
        probes,
        Box::new(BuiltinExecExecutor),
        store.clone(),
        Arc::new(SystemClock),
    );
    let _ = db_path; // SQLite variant constructed directly in the resume test
    (engine, store)
}

/// Ticks until `pred` passes or `timeout_ms` elapses; returns the collected
/// recovery runs.
fn tick_until<F>(engine: &mut Engine, pred: F, timeout_ms: u64) -> Vec<RecoveryRun>
where
    F: Fn(&Engine) -> bool,
{
    let mut runs = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        runs.extend(engine.tick());
        if pred(engine) {
            return runs;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    runs
}

#[test]
fn respawn_worker_end_to_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("worker.pid");
    let pid_file = pid_file.to_str().expect("utf8 path");
    let config = format!(
        r#"
version = 1

[[resources]]
id = "worker"
type = "process"

[resources.probe]
pid_file = "{pid_file}"
interval_ms = 20

[resources.health_policy]
failure_threshold = 1
success_threshold = 1

[[resources.recovery_policy.plans]]
plan_id = "respawn"
applicable_faults = ["process_exited"]
risk = "low"
idempotent = true
timeout_ms = 5000
max_attempts = 1
cooldown_ms = 60_000

[resources.recovery_policy.plans.action]
program = "sh"
args = ["-c", "setsid sh -c 'echo $$ > {pid_file}; exec sleep 30' >/dev/null 2>&1 </dev/null &"]

[resources.recovery_policy.plans.verification]
status_is = "healthy"
"#
    );

    // A live worker first: the engine observes health, nothing runs.
    spawn_worker(pid_file);
    let first_pid = loop {
        if let Some(pid) = read_pid(pid_file) {
            break pid;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let (mut engine, store) = build(&config, None);
    let runs = tick_until(
        &mut engine,
        |e| {
            e.snapshot(&ResourceId::new("worker"))
                .is_some_and(|s| s.status == custos_model::HealthStatus::Healthy)
        },
        2_000,
    );
    assert!(
        runs.is_empty(),
        "healthy worker must not trigger recovery, got {runs:?}"
    );

    // Kill it: the next ticks must detect the death, then respawn and verify.
    let _ = Command::new("kill")
        .arg("-9")
        .arg(first_pid.to_string())
        .status();
    // The recovery run may fire during either phase, so collect across both.
    let mut runs = tick_until(
        &mut engine,
        |e| {
            e.snapshot(&ResourceId::new("worker"))
                .is_some_and(|s| s.status == custos_model::HealthStatus::Unhealthy)
        },
        2_000,
    );
    runs.extend(tick_until(
        &mut engine,
        |e| {
            e.snapshot(&ResourceId::new("worker"))
                .is_some_and(|s| s.status == custos_model::HealthStatus::Healthy)
        },
        5_000,
    ));
    let run = runs
        .iter()
        .find(|r| r.resource_id == ResourceId::new("worker"))
        .expect("a recovery run for the dead worker");
    assert_eq!(run.status, RecoveryRunStatus::Recovered);
    assert_eq!(run.action_attempts.len(), 1);
    // Verification by probe, not exit code: a NEW pid must be alive.
    let respawned = read_pid(pid_file).expect("pid file rewritten");
    assert_ne!(respawned, first_pid);
    assert!(alive(respawned));
    assert!(store.event_types().contains(&"recovery.succeeded"));
    let _ = Command::new("kill")
        .arg("-9")
        .arg(respawned.to_string())
        .status();
}

#[test]
fn dependency_recovery_runs_database_before_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_marker = dir.path().join("db.marker");
    let app_probe_marker = dir.path().join("app.marker");
    let (db_marker, app_probe_marker) = (
        db_marker.to_str().unwrap(),
        app_probe_marker.to_str().unwrap(),
    );
    let config = format!(
        r#"
version = 1

[[resources]]
id = "db"
type = "custom-command"

[resources.probe]
command = ["test", "-f", "{db_marker}"]
interval_ms = 20

[resources.health_policy]
failure_threshold = 1
success_threshold = 1

[[resources.recovery_policy.plans]]
plan_id = "restore-db"
applicable_faults = ["remote_endpoint_unreachable"]
risk = "low"
idempotent = true
timeout_ms = 5000
max_attempts = 1
cooldown_ms = 60_000

[resources.recovery_policy.plans.action]
program = "touch"
args = ["{db_marker}"]

[resources.recovery_policy.plans.verification]
status_is = "healthy"

[[resources]]
id = "app"
type = "custom-command"

[[resources.dependencies]]
resource = "db"
requirement = "required"
condition = {{ status_is = "healthy" }}

[resources.probe]
command = ["test", "-f", "{app_probe_marker}"]
interval_ms = 20

[resources.health_policy]
failure_threshold = 1
success_threshold = 1

[[resources.recovery_policy.plans]]
plan_id = "restore-app"
applicable_faults = ["remote_endpoint_unreachable"]
risk = "low"
idempotent = true
timeout_ms = 5000
max_attempts = 1
cooldown_ms = 60_000

[resources.recovery_policy.plans.action]
# Restoring the app means restoring its health signal (the marker the probe
# checks); we record that the action ran via the attempt log instead.
program = "touch"
args = ["{app_probe_marker}"]

[resources.recovery_policy.plans.verification]
status_is = "healthy"
"#
    );

    let (mut engine, _store) = build(&config, None);
    let runs = tick_until(
        &mut engine,
        |_| {
            std::path::Path::new(app_probe_marker).exists()
                && std::path::Path::new(db_marker).exists()
        },
        5_000,
    );

    let db_run = runs
        .iter()
        .find(|r| r.resource_id == ResourceId::new("db"))
        .expect("db recovered");
    let app_run = runs
        .iter()
        .find(|r| r.resource_id == ResourceId::new("app"))
        .expect("app recovered");
    assert_eq!(db_run.status, RecoveryRunStatus::Recovered);
    assert_eq!(app_run.status, RecoveryRunStatus::Recovered);
    // Topological order: the database run is created before the app run.
    assert!(db_run.started_at_unix_ms <= app_run.started_at_unix_ms);
    // Both actions actually ran and the app action was not blocked.
    assert_eq!(app_run.action_attempts.len(), 1);
}

#[test]
fn crash_resume_over_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("custos.db");
    let config = r#"
version = 1

[[resources]]
id = "svc"
type = "custom-command"

[resources.probe]
command = ["true"]
interval_ms = 20
"#;

    // A previous process died mid-execution: run persisted, outcome NULL.
    {
        let store = Arc::new(custos_store::Store::open(&db).expect("store"));
        let mut run = RecoveryRun::new(
            RecoveryRunId::new("run-crash"),
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
        store.save_attempt(&run.id, &run.action_attempts[0]);
    }

    // Restart with the same database: the attempt must become
    // OutcomeUnknown, never blindly re-executed (§6.6).
    let config = load(config).expect("config");
    let mut probes: HashMap<ResourceId, Box<dyn Probe>> = HashMap::new();
    for resource in &config.resources {
        probes.insert(
            resource.id.clone(),
            custos_probe::factory::build_probe(resource).expect("probe"),
        );
    }
    let store = Arc::new(custos_store::Store::open(&db).expect("reopen"));
    let mut engine = Engine::new(
        &config,
        probes,
        Box::new(|_: &custos_model::ActionSpec, _| custos_model::ActionOutcome::Failed),
        store.clone(),
        Arc::new(SystemClock),
    );
    assert_eq!(engine.resume_unfinished(), 1);
    let resumed = store
        .run(&RecoveryRunId::new("run-crash"))
        .expect("query")
        .expect("present");
    assert_eq!(resumed.status, RecoveryRunStatus::OutcomeUnknown);
    assert_eq!(
        resumed.action_attempts[0].outcome,
        Some(custos_model::ActionOutcome::OutcomeUnknown)
    );
}

#[test]
fn action_spec_runs_the_configured_program() {
    // Sanity for the builtin executor path used above: a failing program
    // maps to Failed, a sleeping one times out.
    let exec = BuiltinExecExecutor;
    assert_eq!(
        exec.execute(
            &ActionSpec {
                program: "false".to_owned(),
                args: vec![]
            },
            Duration::from_millis(1_000)
        ),
        custos_model::ActionOutcome::Failed
    );
    assert_eq!(
        exec.execute(
            &ActionSpec {
                program: "sleep".to_owned(),
                args: vec!["5".to_owned()]
            },
            Duration::from_millis(100)
        ),
        custos_model::ActionOutcome::TimedOut
    );
}

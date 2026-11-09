use super::*;
use crate::config::RetentionConfig;
use crate::migrations::MIGRATIONS;
use custos_model::ids::{ActionId, ResourceId};
use custos_model::{ActionOutcome, RecoveryRunStatus};
use std::sync::Arc;

fn run(id: &str, status: RecoveryRunStatus) -> RecoveryRun {
    let mut run = RecoveryRun::new(
        RecoveryRunId::new(id),
        ResourceId::new("svc"),
        "failure threshold reached",
        1_000,
    );
    run.status = status;
    run
}

fn attempt(action_id: &str, outcome: Option<ActionOutcome>) -> ActionAttempt {
    ActionAttempt {
        action_id: ActionId::new(action_id),
        plan_id: "restart".to_owned(),
        attempt_no: 1,
        outcome,
        started_at_unix_ms: 1_000,
        finished_at_unix_ms: outcome.map(|_| 1_100),
    }
}

/// Executes raw SQL; only for seeding tables the engine does not write
/// yet (incidents, notifications, probe results).
fn exec(store: &Store, sql: &str) {
    store
        .block(|pool| async move { query(sql).execute(&pool).await.map(|_| ()) })
        .expect("test sql");
}

fn count(store: &Store, table: &str) -> i64 {
    store
        .block(|pool| async move {
            query(&format!("SELECT COUNT(*) AS n FROM {table}"))
                .fetch_one(&pool)
                .await
                .map(|row| row.get::<i64, _>("n"))
        })
        .expect("count")
}

#[test]
fn persists_and_reopens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("custos.db");
    {
        let store = Store::open(&path).expect("open");
        let mut r = run("run-1", RecoveryRunStatus::Executing);
        r.action_attempts.push(attempt("action-1", None));
        store.save_run(&r);
        store.save_attempt(&r.id, &r.action_attempts[0]);
    }
    // Simulate a restart: reopen the same file.
    let store = Store::open(&path).expect("reopen");
    let unfinished = store.unfinished_runs();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].id.as_str(), "run-1");
    assert_eq!(unfinished[0].action_attempts.len(), 1);
    assert_eq!(unfinished[0].action_attempts[0].outcome, None);
}

#[test]
fn attempt_outcome_updates_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("custos.db")).expect("open");
    let r = run("run-2", RecoveryRunStatus::Executing);
    store.save_run(&r);
    // §7.3: persist as Executing (outcome NULL) first, then the result.
    store.save_attempt(&r.id, &attempt("action-1", None));
    store.save_attempt(&r.id, &attempt("action-1", Some(ActionOutcome::Succeeded)));

    let counts = store.counts().expect("counts");
    assert_eq!(counts.attempts, 1, "upsert must not duplicate the row");
    let loaded = store.run(&r.id).expect("run").expect("present");
    assert_eq!(
        loaded.action_attempts[0].outcome,
        Some(ActionOutcome::Succeeded)
    );
}

#[test]
fn terminal_runs_excluded_from_unfinished() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("custos.db")).expect("open");
    store.save_run(&run("run-a", RecoveryRunStatus::Escalated));
    store.save_run(&run("run-b", RecoveryRunStatus::Verifying));
    assert_eq!(store.unfinished_runs().len(), 1);
}

#[test]
fn audit_events_are_recorded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("custos.db")).expect("open");
    let resource = ResourceId::new("svc");
    let run_id = RecoveryRunId::new("run-3");
    store.emit(EngineEvent::RecoveryStarted {
        run_id: run_id.clone(),
        resource_id: resource.clone(),
        at_unix_ms: 1_234,
    });
    store.save_run(&run("run-3", RecoveryRunStatus::Observing));
    let rows = store.recent_audit(10).expect("audit");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_type, "recovery.started");
    assert_eq!(rows[0].run_id.as_deref(), Some("run-3"));
    assert_eq!(rows[0].resource_id, "svc");
    let counts = store.counts().expect("counts");
    assert_eq!(counts.audit_events, 1);
}

#[test]
fn engine_round_trip_through_store() {
    // The engine's memory-store semantics hold against SQLite too.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(Store::open(dir.path().join("custos.db")).expect("open"));
    store.save_run(&run("run-4", RecoveryRunStatus::Executing));
    store.save_attempt(&RecoveryRunId::new("run-4"), &attempt("action-9", None));
    let unfinished = store.unfinished_runs();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].action_attempts[0].outcome, None);
}

#[test]
fn corrupt_outcome_text_is_an_error_not_silent_none() {
    // C2: an outcome cell that is present but unparsable is a corrupt row,
    // not "no outcome recorded". The typed read reports it like a corrupt
    // run payload; the crash-recovery input keeps the conservative None
    // (OutcomeUnknown downstream) with the raw text logged, never silent.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("custos.db")).expect("open");
    store.save_run(&run("run-c2", RecoveryRunStatus::Executing));
    exec(
        &store,
        "INSERT INTO action_attempts \
         (run_id, action_id, plan_id, attempt_no, outcome, started_at_unix_ms, finished_at_unix_ms) \
         VALUES ('run-c2', 'action-x', 'restart', 1, 'not-json{\"', 1000, 1100)",
    );
    let err = store
        .run(&RecoveryRunId::new("run-c2"))
        .expect_err("corrupt outcome must surface");
    assert!(err.to_string().contains("corrupt row"), "{err}");
    // Crash-recovery input: the run is not dropped; the corrupt attempt
    // reads as unknown (the legal conservative transition) with the raw
    // text reported, never silently confused with a crash mid-run.
    let unfinished = store.unfinished_runs();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].action_attempts[0].outcome, None);
}

#[test]
fn migrations_apply_once_and_are_recorded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("custos.db");
    {
        let store = Store::open(&path).expect("open");
        let applied = store.applied_migrations().expect("applied");
        assert_eq!(applied.len(), MIGRATIONS.len());
        assert_eq!(applied[0].1, "recovery-state-and-audit");
        assert_eq!(applied[1].1, "full-core-schema");
    }
    // Reopen: nothing re-applies (the recorded versions match), and the
    // count stays the same.
    let store = Store::open(&path).expect("reopen");
    assert_eq!(store.applied_migrations().unwrap().len(), MIGRATIONS.len());
    // V2 tables exist and are queryable.
    assert_eq!(count(&store, "probe_results"), 0);
    assert_eq!(count(&store, "notification_queue"), 0);
    assert_eq!(count(&store, "incidents"), 0);
}

#[test]
fn legacy_pre_framework_database_upgrades_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy.db");
    // A custos built before the migration framework: V1 tables, no
    // schema_migrations, and a run that must survive the upgrade.
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);
        rt.block_on(async {
                let pool = SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect_with(options)
                    .await
                    .expect("pool");
                query(MIGRATIONS[0].sql).execute(&pool).await.expect("v1");
                let payload =
                    serde_json::to_string(&run("legacy-run", RecoveryRunStatus::Recovered))
                        .expect("payload");
                query(
                    "INSERT INTO recovery_runs \
                     (id, resource_id, status, trigger, started_at_unix_ms, updated_at_unix_ms, payload) \
                     VALUES ('legacy-run', 'svc', 'recovered', 'old', 1, 1, ?)",
                )
                .bind(payload)
                .execute(&pool)
                .await
                .expect("seed");
            });
    }
    let store = Store::open(&path).expect("upgrade");
    // V1 was a no-op re-run, V2 applied, data intact.
    assert_eq!(store.applied_migrations().unwrap().len(), MIGRATIONS.len());
    assert!(store
        .run(&RecoveryRunId::new("legacy-run"))
        .expect("run")
        .is_some());
}

#[test]
fn open_with_configures_synchronous_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = StoreConfig {
        synchronous: SynchronousPolicy::Normal,
        retention: RetentionConfig::default(),
    };
    let store = Store::open_with(dir.path().join("custos.db"), &config).expect("open");
    // PRAGMA synchronous reports an integer level (0=OFF,1=NORMAL,2=FULL).
    let mode: i64 = store
        .block(|pool| async move {
            query("PRAGMA synchronous")
                .fetch_one(&pool)
                .await
                .map(|row| row.get::<i64, _>(0))
        })
        .expect("pragma");
    assert_eq!(mode, 1, "NORMAL is synchronous=1");
    let journal: String = store
        .block(|pool| async move {
            query("PRAGMA journal_mode")
                .fetch_one(&pool)
                .await
                .map(|row| row.get::<String, _>(0))
        })
        .expect("journal");
    assert_eq!(journal.to_lowercase(), "wal");
}

#[test]
fn purge_removes_only_expired_inactive_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let now = 1_000_000_u64;
    let config = StoreConfig {
        synchronous: SynchronousPolicy::Full,
        retention: RetentionConfig {
            audit_retention_ms: Some(10_000),
            probe_detail_retention_ms: Some(10_000),
            run_retention_ms: Some(10_000),
            notification_retention_ms: Some(10_000),
        },
    };
    let store = Store::open_with(dir.path().join("custos.db"), &config).expect("open");
    let old = (now - 20_000) as i64; // expired
    let recent = (now - 1_000) as i64; // inside retention

    // Audit: one old, one recent.
    for (at, id) in [(old, 1), (recent, 2)] {
        exec(&store, &format!(
                "INSERT INTO audit_events (id, at_unix_ms, event_type, severity, resource_id, payload) \
                 VALUES ({id}, {at}, 'recovery.started', 'Info', 'svc', '{{}}')"
            ));
    }
    // Probe results: one old, one recent.
    for (at, id) in [(old, 1), (recent, 2)] {
        exec(&store, &format!(
                "INSERT INTO probe_results (id, resource_id, probe_name, succeeded, observation, collected_at_unix_ms) \
                 VALUES ({id}, 'svc', 'cmd', 1, 'ok', {at})"
            ));
    }
    // Runs: an old terminal run (purgeable) and an old ACTIVE run (must
    // survive despite being expired).
    let mut expired = run("run-old", RecoveryRunStatus::Recovered);
    expired.updated_at_unix_ms = old as u64;
    store.save_run(&expired);
    store.save_attempt(
        &expired.id,
        &attempt("a-old", Some(ActionOutcome::Succeeded)),
    );
    let mut active = run("run-active", RecoveryRunStatus::Executing);
    active.updated_at_unix_ms = old as u64;
    store.save_run(&active);
    store.save_attempt(&active.id, &attempt("a-live", None));
    // Incidents: closed+old (purgeable), open+old (must survive).
    exec(&store, &format!(
            "INSERT INTO incidents (id, resource_id, fault_code, summary, opened_at_unix_ms, closed_at_unix_ms) \
             VALUES ('inc-closed', 'svc', 'process_exited', 'closed', {old}, {old})"
        ));
    exec(
        &store,
        &format!(
            "INSERT INTO incidents (id, resource_id, fault_code, summary, opened_at_unix_ms) \
             VALUES ('inc-open', 'svc', 'process_exited', 'still open', {old})"
        ),
    );
    // Notifications: delivered+old (purgeable), undelivered+old (must
    // survive), with an attempt history row.
    exec(&store, &format!(
            "INSERT INTO notification_queue (id, resource_id, channel, payload, created_at_unix_ms, delivered_at_unix_ms) \
             VALUES (1, 'svc', 'desktop', '{{}}', {old}, {old})"
        ));
    exec(&store, &format!(
            "INSERT INTO notification_queue (id, resource_id, channel, payload, created_at_unix_ms) \
             VALUES (2, 'svc', 'desktop', '{{}}', {old})"
        ));
    exec(
        &store,
        &format!(
            "INSERT INTO notification_attempts (notification_id, outcome, detail, at_unix_ms) \
             VALUES (1, 'delivered', 'ok', {old})"
        ),
    );

    let report = store.purge_retention(now).expect("purge");
    assert_eq!(report.audit_events, 1);
    assert_eq!(report.probe_results, 1);
    assert_eq!(report.runs, 1);
    assert_eq!(report.attempts, 1);
    assert_eq!(report.incidents, 1);
    assert_eq!(report.delivered_notifications, 1);
    assert_eq!(report.notification_attempts, 1);

    // Survivors: active run + attempt, open incident, undelivered
    // notification, recent audit/probe rows.
    assert_eq!(count(&store, "recovery_runs"), 1);
    assert_eq!(count(&store, "action_attempts"), 1);
    assert_eq!(count(&store, "incidents"), 1);
    assert_eq!(count(&store, "notification_queue"), 1);
    assert_eq!(count(&store, "audit_events"), 1);
    assert_eq!(count(&store, "probe_results"), 1);
    let survivors = store.list_runs(10).expect("runs");
    assert_eq!(survivors[0].id, "run-active");
}

/// Fault injection §16.3 case 8 (store side): an unwritable location
/// must surface a clear error at open, never a silent no-store.
#[test]
fn open_failure_on_missing_directory_is_reported() {
    let err = match Store::open("/nonexistent-custos-dir/custos.db") {
        Err(err) => err,
        Ok(_) => panic!("opened a database in a nonexistent directory"),
    };
    assert!(err.to_string().contains("database error"));
}

#[test]
fn list_runs_orders_newest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("custos.db")).expect("open");
    let mut first = run("run-first", RecoveryRunStatus::Recovered);
    first.updated_at_unix_ms = 100;
    let mut second = run("run-second", RecoveryRunStatus::Escalated);
    second.updated_at_unix_ms = 200;
    store.save_run(&first);
    store.save_run(&second);
    let rows = store.list_runs(10).expect("runs");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "run-second");
    assert_eq!(rows[1].id, "run-first");
    let limited = store.list_runs(1).expect("limited");
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].id, "run-second");
}

fn notification_event(event_id: &str) -> custos_model::NotificationEvent {
    custos_model::NotificationEvent::new(
        custos_model::ids::EventId::new(event_id),
        custos_model::EventType::ResourceUnhealthy,
        custos_model::Severity::Critical,
        "db down",
        1_700_000_000_000,
    )
    .with_resource_id(ResourceId::new("db"))
}

#[test]
fn notification_queue_lifecycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("q.db")).expect("open");

    let id = store
        .enqueue_notification("ops-webhook", &notification_event("evt-1"), 1_000)
        .expect("enqueue");
    assert!(id > 0);

    // Nothing due before the next attempt time... it's pending, so due now.
    let due = store.claim_due_notifications(1_000, 10).expect("claim");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, id);
    assert_eq!(due[0].channel, "ops-webhook");
    assert_eq!(due[0].state, custos_model::NotificationState::Sending);
    assert_eq!(due[0].event.event_id.as_str(), "evt-1");
    assert_eq!(
        due[0].event.resource_id.as_ref().map(|r| r.as_str()),
        Some("db")
    );

    // A second claim at the same time must not re-claim the sending row.
    assert!(store.claim_due_notifications(1_000, 10).unwrap().is_empty());

    // Deliver it.
    store
        .mark_notification_delivered(id, 1_200)
        .expect("delivered");
    let rows = store.list_notifications(10).expect("list");
    assert_eq!(rows[0].state, custos_model::NotificationState::Delivered);
    assert_eq!(rows[0].delivered_at_unix_ms, Some(1_200));
}

#[test]
fn notification_queue_retry_and_dead_letter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("q.db")).expect("open");
    let event = notification_event("evt-2");

    let id = store
        .enqueue_notification("mail", &event, 1_000)
        .expect("enqueue");
    store.claim_due_notifications(1_000, 10).unwrap();
    store
        .mark_notification_retry(id, 5_000, "smtp: transient", 1_000)
        .expect("retry");
    let rows = store.list_notifications(10).unwrap();
    assert_eq!(
        rows[0].state,
        custos_model::NotificationState::RetryScheduled
    );
    assert_eq!(rows[0].attempts, 1);
    assert_eq!(rows[0].next_attempt_at_unix_ms, Some(5_000));
    assert_eq!(rows[0].last_error.as_deref(), Some("smtp: transient"));

    // Not due until 5000.
    assert!(store.claim_due_notifications(4_999, 10).unwrap().is_empty());
    assert_eq!(store.claim_due_notifications(5_000, 10).unwrap().len(), 1);

    // Exhaust: mark dead.
    store
        .mark_notification_dead(id, "smtp: permanent", 5_100)
        .unwrap();
    let rows = store.list_notifications(10).unwrap();
    assert_eq!(rows[0].state, custos_model::NotificationState::DeadLetter);
    assert!(rows[0].last_error.as_deref().unwrap().contains("permanent"));

    // Manual retry resurrects it as pending.
    store.retry_notification(id, 5_200).unwrap();
    let rows = store.list_notifications(10).unwrap();
    assert_eq!(rows[0].state, custos_model::NotificationState::Pending);
    assert_eq!(rows[0].next_attempt_at_unix_ms, None);
}

#[test]
fn notification_counts_aggregate_per_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("q.db")).expect("open");
    let a = store
        .enqueue_notification("c1", &notification_event("evt-a"), 1_000)
        .unwrap();
    let b = store
        .enqueue_notification("c2", &notification_event("evt-b"), 1_000)
        .unwrap();
    store.claim_due_notifications(1_000, 10).unwrap();
    store.mark_notification_delivered(a, 1_100).unwrap();
    store
        .mark_notification_retry(b, 9_000, "timeout", 1_100)
        .unwrap();

    let counts = store.notification_counts().unwrap();
    assert_eq!(counts.delivered, 1);
    assert_eq!(counts.retry_scheduled, 1);
    assert_eq!(counts.total(), 2);
}

#[test]
fn delivered_notification_is_not_flipped_back_by_stale_retry() {
    // A superseded claim must not resurrect a delivered notification:
    // mark_retry carries the same `sending` lease guard as mark_delivered.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("q.db")).expect("open");
    let id = store
        .enqueue_notification("mail", &notification_event("evt-3"), 1_000)
        .unwrap();
    store.claim_due_notifications(1_000, 10).unwrap();
    store.mark_notification_delivered(id, 1_100).unwrap();
    // The losing pass of a duplicate delivery tries to retry the row.
    store
        .mark_notification_retry(id, 9_000, "stale retry", 1_200)
        .unwrap();
    let rows = store.list_notifications(10).unwrap();
    assert_eq!(
        rows[0].state,
        custos_model::NotificationState::Delivered,
        "delivered row must not flip back to retry"
    );
    assert_eq!(rows[0].attempts, 0, "no extra attempt counted");
}

//! The SQLite [`RunStore`]: a sync facade over a sqlx pool. The engine stays
//! synchronous; each call blocks on a current-thread tokio runtime owned by
//! the store.

use crate::config::{PurgeReport, RetentionConfig, StoreConfig, SynchronousPolicy};
use crate::migrations::migrate;
use custos_engine::{EngineEvent, RunStore};
use custos_model::ids::RecoveryRunId;
use custos_model::{ActionAttempt, RecoveryRun, VerificationResult};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{query, query_as, Row, SqlitePool};
use std::future::Future;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("corrupt row for `{0}`: {1}")]
    Corrupt(String, #[source] serde_json::Error),
}

/// Row shape of one persisted attempt.
type AttemptRow = (String, String, i64, Option<String>, i64, Option<i64>);

/// Terminal statuses excluded from `unfinished_runs` (§6.1) and eligible for
/// retention purging (§7.4).
const TERMINAL: &str =
    "('recovered','escalated','suppressed','cancelled','interrupted','outcome_unknown')";

pub struct Store {
    pool: SqlitePool,
    /// Serializes `block_on` calls; a current-thread runtime is not reentrant.
    rt_lock: Mutex<tokio::runtime::Runtime>,
    retention: RetentionConfig,
}

impl Store {
    /// Opens (creating if needed) with the default config: WAL, `synchronous
    /// = FULL`, nothing ever purged.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with(path, &StoreConfig::default())
    }

    /// Opens (creating if needed) the database at `path` with WAL enabled,
    /// the configured `synchronous` policy (§7.1), and all pending schema
    /// migrations applied (§7.2).
    pub fn open_with(path: impl AsRef<Path>, config: &StoreConfig) -> Result<Self, StoreError> {
        let synchronous = match config.synchronous {
            SynchronousPolicy::Off => SqliteSynchronous::Off,
            SynchronousPolicy::Normal => SqliteSynchronous::Normal,
            SynchronousPolicy::Full => SqliteSynchronous::Full,
        };
        let options = SqliteConnectOptions::new()
            .filename(path.as_ref())
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(synchronous)
            .create_if_missing(true);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| StoreError::Sqlx(sqlx::Error::Io(std::io::Error::other(e))))?;
        let pool = rt.block_on(
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options),
        )?;
        let store = Self {
            pool,
            rt_lock: Mutex::new(rt),
            retention: config.retention,
        };
        store.block(|pool| async move { migrate(pool).await.map(|_| ()) })?;
        Ok(store)
    }

    /// Schema versions currently applied, for `custos status` and tests.
    pub fn applied_migrations(&self) -> Result<Vec<(i64, String, i64)>, StoreError> {
        Ok(self.block(|pool| async move {
            query_as::<_, (i64, String, i64)>(
                "SELECT version, name, applied_at_unix_ms \
                     FROM schema_migrations ORDER BY version",
            )
            .fetch_all(&pool)
            .await
        })?)
    }

    /// Runs `f` on the store's runtime. Futures need not be `Send`:
    /// the runtime is single-threaded and the pool has one connection.
    fn block<T, F, Fut>(&self, f: F) -> Result<T, sqlx::Error>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = Result<T, sqlx::Error>>,
    {
        let rt = self.rt_lock.lock().unwrap_or_else(|e| e.into_inner());
        rt.block_on(f(self.pool.clone()))
    }

    /// Row counts per table, for `custos status` and tests.
    pub fn counts(&self) -> Result<Counts, StoreError> {
        self.block(|pool| async move {
            let runs: i64 = query("SELECT COUNT(*) AS n FROM recovery_runs")
                .fetch_one(&pool)
                .await?
                .get("n");
            let attempts: i64 = query("SELECT COUNT(*) AS n FROM action_attempts")
                .fetch_one(&pool)
                .await?
                .get("n");
            let verifications: i64 = query("SELECT COUNT(*) AS n FROM verification_results")
                .fetch_one(&pool)
                .await?
                .get("n");
            let audit_events: i64 = query("SELECT COUNT(*) AS n FROM audit_events")
                .fetch_one(&pool)
                .await?
                .get("n");
            Ok(Counts {
                runs: runs as u64,
                attempts: attempts as u64,
                verifications: verifications as u64,
                audit_events: audit_events as u64,
            })
        })
        .map_err(Into::into)
    }

    /// Recent audit events, newest first (for the CLI).
    pub fn recent_audit(&self, limit: u32) -> Result<Vec<AuditRow>, StoreError> {
        self.block(|pool| async move {
            let rows: Vec<(i64, i64, String, String, String, Option<String>)> = query_as(
                "SELECT id, at_unix_ms, event_type, severity, resource_id, run_id \
                     FROM audit_events ORDER BY id DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(
                    |(id, at, event_type, severity, resource_id, run_id)| AuditRow {
                        id: id as u64,
                        at_unix_ms: at as u64,
                        event_type,
                        severity,
                        resource_id,
                        run_id,
                    },
                )
                .collect())
        })
        .map_err(Into::into)
    }

    /// Recent recovery runs, newest first (for `custos runs`).
    pub fn list_runs(&self, limit: u32) -> Result<Vec<RunRow>, StoreError> {
        self.block(|pool| async move {
            let rows: Vec<(String, String, String, String, i64, i64)> = query_as(
                "SELECT id, resource_id, status, trigger, started_at_unix_ms, updated_at_unix_ms \
                 FROM recovery_runs ORDER BY updated_at_unix_ms DESC, id DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(
                    |(id, resource_id, status, trigger, started, updated)| RunRow {
                        id,
                        resource_id,
                        status,
                        trigger,
                        started_at_unix_ms: started as u64,
                        updated_at_unix_ms: updated as u64,
                    },
                )
                .collect())
        })
        .map_err(Into::into)
    }

    /// Recent incidents, newest first (for `custos incidents`).
    pub fn list_incidents(&self, limit: u32) -> Result<Vec<IncidentRow>, StoreError> {
        self.block(|pool| async move {
            let rows: Vec<(String, String, String, String, i64, Option<i64>)> = query_as(
                "SELECT id, resource_id, fault_code, summary, opened_at_unix_ms, closed_at_unix_ms \
                 FROM incidents ORDER BY opened_at_unix_ms DESC, id DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(
                    |(id, resource_id, fault_code, summary, opened, closed)| IncidentRow {
                        id,
                        resource_id,
                        fault_code,
                        summary,
                        opened_at_unix_ms: opened as u64,
                        closed_at_unix_ms: closed.map(|t| t as u64),
                    },
                )
                .collect())
        })
        .map_err(Into::into)
    }

    /// One run by id, with attempts reattached.
    pub fn run(&self, id: &RecoveryRunId) -> Result<Option<RecoveryRun>, StoreError> {
        let id = id.as_str().to_owned();
        let bound_id = id.clone();
        let found = self.block(|pool| async move {
            query_as::<_, (String,)>("SELECT payload FROM recovery_runs WHERE id = ?")
                .bind(&bound_id)
                .fetch_optional(&pool)
                .await
        })?;
        match found {
            None => Ok(None),
            Some((payload,)) => {
                let mut run: RecoveryRun = serde_json::from_str(&payload)
                    .map_err(|err| StoreError::Corrupt(id.clone(), err))?;
                run.action_attempts = self.attempts_of(&id)?;
                Ok(Some(run))
            }
        }
    }

    fn attempts_of(&self, run_id: &str) -> Result<Vec<ActionAttempt>, StoreError> {
        let run_id = run_id.to_owned();
        let rows = self.block(|pool| async move {
            let rows: Vec<AttemptRow> = query_as(
                "SELECT action_id, plan_id, attempt_no, outcome, started_at_unix_ms, finished_at_unix_ms \
                 FROM action_attempts WHERE run_id = ? ORDER BY started_at_unix_ms, attempt_no",
            )
            .bind(&run_id)
            .fetch_all(&pool)
            .await?;
            sqlx::Result::Ok(rows)
        })?;
        Ok(rows
            .into_iter()
            .map(
                |(action_id, plan_id, attempt_no, outcome, started, finished)| ActionAttempt {
                    action_id: action_id.into(),
                    plan_id,
                    attempt_no: attempt_no as u32,
                    // NULL outcome = persisted as Executing, then the process
                    // died: the real outcome is unknown (§7.3 step 6).
                    outcome: outcome
                        .as_deref()
                        .and_then(|o| serde_json::from_str(o).ok()),
                    started_at_unix_ms: started as u64,
                    finished_at_unix_ms: finished.map(|t| t as u64),
                },
            )
            .collect())
    }

    /// Applies the retention policy (§7.4) as of `now_unix_ms`. Active state
    /// is never touched: open incidents, non-terminal runs and undelivered
    /// notifications are excluded by the WHERE clauses, whatever their age.
    pub fn purge_retention(&self, now_unix_ms: u64) -> Result<PurgeReport, StoreError> {
        let retention = self.retention;
        self.block(|pool| async move {
            let mut report = PurgeReport::default();

            if let Some(cutoff) = retention.audit_cutoff(now_unix_ms) {
                report.audit_events = query("DELETE FROM audit_events WHERE at_unix_ms < ?")
                    .bind(cutoff)
                    .execute(&pool)
                    .await?
                    .rows_affected();
            }

            if let Some(cutoff) = retention.probe_cutoff(now_unix_ms) {
                report.probe_results = query("DELETE FROM probe_results WHERE collected_at_unix_ms < ?")
                    .bind(cutoff)
                    .execute(&pool)
                    .await?
                    .rows_affected();
            }

            if let Some(run_cutoff) = retention.run_cutoff(now_unix_ms) {
                // Attempts first: they reference the runs being removed.
                report.attempts = query(&format!(
                    "DELETE FROM action_attempts WHERE run_id IN (\
                     SELECT id FROM recovery_runs \
                     WHERE updated_at_unix_ms < ? AND status IN {TERMINAL})"
                ))
                .bind(run_cutoff)
                .execute(&pool)
                .await?
                .rows_affected();
                report.verifications =
                    query("DELETE FROM verification_results WHERE checked_at_unix_ms < ?")
                        .bind(run_cutoff)
                        .execute(&pool)
                        .await?
                        .rows_affected();
                report.runs = query(&format!(
                    "DELETE FROM recovery_runs WHERE updated_at_unix_ms < ? AND status IN {TERMINAL}"
                ))
                .bind(run_cutoff)
                .execute(&pool)
                .await?
                .rows_affected();
                // Closed incidents only; open ones stay until resolved.
                report.incidents =
                    query("DELETE FROM incidents WHERE closed_at_unix_ms IS NOT NULL AND closed_at_unix_ms < ?")
                        .bind(run_cutoff)
                        .execute(&pool)
                        .await?
                        .rows_affected();
            }

            if let Some(cutoff) = retention.notification_cutoff(now_unix_ms) {
                report.notification_attempts =
                    query("DELETE FROM notification_attempts WHERE at_unix_ms < ?")
                        .bind(cutoff)
                        .execute(&pool)
                        .await?
                        .rows_affected();
                // Delivered only; undelivered notifications must survive
                // offline periods of any length (§7.4).
                report.delivered_notifications = query(
                    "DELETE FROM notification_queue \
                     WHERE delivered_at_unix_ms IS NOT NULL AND delivered_at_unix_ms < ?",
                )
                .bind(cutoff)
                .execute(&pool)
                .await?
                .rows_affected();
            }

            Ok(report)
        })
        .map_err(Into::into)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub runs: u64,
    pub attempts: u64,
    pub verifications: u64,
    pub audit_events: u64,
}

#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: u64,
    pub at_unix_ms: u64,
    pub event_type: String,
    pub severity: String,
    pub resource_id: String,
    pub run_id: Option<String>,
}

/// One recovery run row, for `custos runs`.
#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: String,
    pub resource_id: String,
    pub status: String,
    pub trigger: String,
    pub started_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

/// One incident row, for `custos incidents`.
#[derive(Debug, Clone)]
pub struct IncidentRow {
    pub id: String,
    pub resource_id: String,
    pub fault_code: String,
    pub summary: String,
    pub opened_at_unix_ms: u64,
    pub closed_at_unix_ms: Option<u64>,
}

impl RunStore for Store {
    fn save_run(&self, run: &RecoveryRun) {
        let payload = serde_json::to_string(run).unwrap_or_else(|_| "{}".to_owned());
        let incident = run.incident_id.as_ref().map(|i| i.as_str().to_owned());
        // Errors are reported, not propagated: the RunStore contract is
        // fire-and-forget and the engine must not stall on persistence.
        if let Err(err) = self.block(|pool| {
            let payload = payload.clone();
            let incident = incident.clone();
            async move {
                query(
                    "INSERT INTO recovery_runs \
                     (id, resource_id, incident_id, status, trigger, started_at_unix_ms, updated_at_unix_ms, payload) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                     ON CONFLICT(id) DO UPDATE SET status=?4, incident_id=?3, updated_at_unix_ms=?7, payload=?8",
                )
                .bind(run.id.as_str())
                .bind(run.resource_id.as_str())
                .bind(&incident)
                .bind(status_key(&run.status))
                .bind(&run.trigger)
                .bind(run.started_at_unix_ms as i64)
                .bind(run.updated_at_unix_ms as i64)
                .bind(&payload)
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", run = run.id.as_str(), error = %err, "save_run failed");
        }
    }

    fn save_attempt(&self, run_id: &RecoveryRunId, attempt: &ActionAttempt) {
        let outcome = attempt
            .outcome
            .map(|o| serde_json::to_string(&o).unwrap_or_default());
        if let Err(err) = self.block(|pool| {
            let outcome = outcome.clone();
            let attempt = attempt.clone();
            async move {
                query(
                    "INSERT INTO action_attempts \
                     (run_id, action_id, plan_id, attempt_no, outcome, started_at_unix_ms, finished_at_unix_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                     ON CONFLICT(run_id, action_id) DO UPDATE SET outcome=?5, finished_at_unix_ms=?7",
                )
                .bind(run_id.as_str())
                .bind(attempt.action_id.as_str())
                .bind(&attempt.plan_id)
                .bind(attempt.attempt_no as i64)
                .bind(&outcome)
                .bind(attempt.started_at_unix_ms as i64)
                .bind(attempt.finished_at_unix_ms.map(|t| t as i64))
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", run = run_id.as_str(), error = %err, "save_attempt failed");
        }
    }

    fn save_verification(&self, result: &VerificationResult) {
        let evidence = serde_json::to_string(&result.evidence_refs).unwrap_or_default();
        if let Err(err) = self.block(|pool| {
            let result = result.clone();
            let evidence = evidence.clone();
            async move {
                query(
                    "INSERT INTO verification_results \
                     (resource_id, passed, summary, evidence_refs, checked_at_unix_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(result.resource_id.as_str())
                .bind(result.passed)
                .bind(&result.summary)
                .bind(&evidence)
                .bind(result.checked_at_unix_ms as i64)
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", resource = result.resource_id.as_str(), error = %err, "save_verification failed");
        }
    }

    fn emit(&self, event: EngineEvent) {
        let severity = format!("{:?}", event.severity());
        let run_id = run_id_of(&event).map(|s| s.to_owned());
        let payload = format!("{{\"event_type\":\"{}\"}}", event.event_type());
        if let Err(err) = self.block(|pool| {
            let severity = severity.clone();
            let run_id = run_id.clone();
            let payload = payload.clone();
            let event = event.clone();
            async move {
                query(
                    "INSERT INTO audit_events (at_unix_ms, event_type, severity, resource_id, run_id, payload) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .bind(event.at_unix_ms() as i64)
                .bind(event.event_type())
                .bind(&severity)
                .bind(event.resource_id().as_str())
                .bind(&run_id)
                .bind(&payload)
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", event = event.event_type(), error = %err, "audit emit failed");
        }
    }

    /// Runs that never reached a terminal state, with their attempts
    /// reattached — the crash-recovery input (§7.3 step 6).
    fn unfinished_runs(&self) -> Vec<RecoveryRun> {
        let result = self.block(|pool| async move {
            let payloads: Vec<(String, String)> = query_as::<_, (String, String)>(&format!(
                "SELECT id, payload FROM recovery_runs WHERE status NOT IN {TERMINAL}"
            ))
            .fetch_all(&pool)
            .await?;
            let mut runs = Vec::new();
            for (id, payload) in payloads {
                let mut run: RecoveryRun = match serde_json::from_str(&payload) {
                    Ok(run) => run,
                    Err(err) => {
                        tracing::error!(target: "custos::store", run = %id, error = %err, "corrupt run row skipped");
                        continue;
                    }
                };
                let attempts: Vec<AttemptRow> =
                    query_as(
                        "SELECT action_id, plan_id, attempt_no, outcome, started_at_unix_ms, finished_at_unix_ms \
                         FROM action_attempts WHERE run_id = ? ORDER BY started_at_unix_ms, attempt_no",
                    )
                    .bind(&id)
                    .fetch_all(&pool)
                    .await?;
                run.action_attempts = attempts
                    .into_iter()
                    .map(|(action_id, plan_id, attempt_no, outcome, started, finished)| {
                        ActionAttempt {
                            action_id: action_id.into(),
                            plan_id,
                            attempt_no: attempt_no as u32,
                            outcome: outcome.as_deref().and_then(|o| serde_json::from_str(o).ok()),
                            started_at_unix_ms: started as u64,
                            finished_at_unix_ms: finished.map(|t| t as u64),
                        }
                    })
                    .collect();
                runs.push(run);
            }
            sqlx::Result::Ok(runs)
        });
        match result {
            Ok(runs) => runs,
            Err(err) => {
                tracing::error!(target: "custos::store", error = %err, "unfinished_runs failed");
                Vec::new()
            }
        }
    }
}

/// Serde serializes enum variants with quotes; the status column stores the
/// bare snake_case key so SQL comparisons (NOT IN terminal) work.
fn status_key(status: &custos_model::RecoveryRunStatus) -> String {
    serde_json::to_string(status)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}

fn run_id_of(event: &EngineEvent) -> Option<&str> {
    match event {
        EngineEvent::RecoveryStarted { run_id, .. }
        | EngineEvent::ActionStarted { run_id, .. }
        | EngineEvent::ActionFinished { run_id, .. }
        | EngineEvent::VerificationRecorded { run_id, .. }
        | EngineEvent::RecoverySucceeded { run_id, .. }
        | EngineEvent::RecoveryExhausted { run_id, .. }
        | EngineEvent::OutcomeUnknown { run_id, .. }
        | EngineEvent::DependencyUnsatisfied { run_id, .. } => Some(run_id.as_str()),
    }
}

#[cfg(test)]
mod tests {
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
}

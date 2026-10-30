//! The SQLite [`RunStore`]: a sync facade over a sqlx pool. The engine stays
//! synchronous; each call blocks on a current-thread tokio runtime owned by
//! the store.

use custos_engine::{EngineEvent, RunStore};
use custos_model::ids::RecoveryRunId;
use custos_model::{ActionAttempt, RecoveryRun, VerificationResult};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
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

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS recovery_runs (
    id                 TEXT PRIMARY KEY,
    resource_id        TEXT NOT NULL,
    incident_id        TEXT,
    status             TEXT NOT NULL,
    trigger            TEXT NOT NULL,
    started_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    payload            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_recovery_runs_status ON recovery_runs(status);

CREATE TABLE IF NOT EXISTS action_attempts (
    run_id              TEXT NOT NULL REFERENCES recovery_runs(id),
    action_id           TEXT NOT NULL,
    plan_id             TEXT NOT NULL,
    attempt_no          INTEGER NOT NULL,
    outcome             TEXT,
    started_at_unix_ms  INTEGER NOT NULL,
    finished_at_unix_ms INTEGER,
    PRIMARY KEY (run_id, action_id)
);

CREATE TABLE IF NOT EXISTS verification_results (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    resource_id        TEXT NOT NULL,
    passed             INTEGER NOT NULL,
    summary            TEXT NOT NULL,
    evidence_refs      TEXT NOT NULL,
    checked_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at_unix_ms  INTEGER NOT NULL,
    event_type  TEXT NOT NULL,
    severity    TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    run_id      TEXT,
    payload     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_events_at ON audit_events(at_unix_ms);
";

/// Row shape of one persisted attempt.
type AttemptRow = (String, String, i64, Option<String>, i64, Option<i64>);

/// Terminal statuses excluded from `unfinished_runs` (§6.1).
const TERMINAL: &str =
    "('recovered','escalated','suppressed','cancelled','interrupted','outcome_unknown')";

pub struct Store {
    pool: SqlitePool,
    /// Serializes `block_on` calls; a current-thread runtime is not reentrant.
    rt_lock: Mutex<tokio::runtime::Runtime>,
}

impl Store {
    /// Opens (creating if needed) the database at `path` with WAL enabled
    /// and the P0 schema applied idempotently.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path.as_ref())
            .journal_mode(SqliteJournalMode::Wal)
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
        };
        store.block(|pool| async move { query(SCHEMA).execute(&pool).await })?;
        Ok(store)
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
            eprintln!("custos-store: save_run failed: {err}");
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
            eprintln!("custos-store: save_attempt failed: {err}");
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
            eprintln!("custos-store: save_verification failed: {err}");
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
            eprintln!("custos-store: emit failed: {err}");
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
                        eprintln!("custos-store: corrupt run `{id}`: {err}");
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
                eprintln!("custos-store: unfinished_runs failed: {err}");
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
}

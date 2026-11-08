//! Store concern: read-only query API (counts, audit, runs, incidents, probes, latest health).

use super::*;

impl Store {
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
    /// Recent probe evidence, newest first.
    pub fn recent_probe_results(&self, limit: u32) -> Result<Vec<ProbeRow>, StoreError> {
        self.block(move |pool| async move {
            let rows: Vec<(String, String, i64, String, i64)> = query_as(
                "SELECT resource_id, probe_name, succeeded, observation, \
                 collected_at_unix_ms FROM probe_results ORDER BY id DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(|(resource, probe, succeeded, observation, at)| ProbeRow {
                    resource_id: resource,
                    probe_name: probe,
                    succeeded: succeeded != 0,
                    observation,
                    collected_at_unix_ms: at as u64,
                })
                .collect())
        })
        .map_err(Into::into)
    }
    /// Latest probe evidence per resource (the persisted health view).
    pub fn latest_health(&self) -> Result<Vec<ProbeRow>, StoreError> {
        self.block(move |pool| async move {
            let rows: Vec<(String, String, i64, String, i64)> = query_as(
                "SELECT resource_id, probe_name, succeeded, observation, \
                 collected_at_unix_ms FROM probe_results p \
                 WHERE id = (SELECT MAX(id) FROM probe_results q WHERE q.resource_id = p.resource_id) \
                 ORDER BY resource_id",
            )
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(|(resource, probe, succeeded, observation, at)| ProbeRow {
                    resource_id: resource,
                    probe_name: probe,
                    succeeded: succeeded != 0,
                    observation,
                    collected_at_unix_ms: at as u64,
                })
                .collect())
        })
        .map_err(Into::into)
    }
}

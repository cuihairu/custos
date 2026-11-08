//! Store concern: run persistence, engine event capture and unfinished-run resume.

use super::*;

impl RunStore for Store {
    fn save_run(&self, run: &RecoveryRun) {
        // Never persist a corrupt payload: `"{}"` looks like a row but has
        // no id, so `unfinished_runs` drops it as corrupt and the recovery
        // is lost silently. Log the serialization failure instead.
        let payload = match serde_json::to_string(run) {
            Ok(payload) => payload,
            Err(err) => {
                tracing::error!(
                    target: "custos::store",
                    run = run.id.as_str(),
                    error = %err,
                    "serialize recovery run failed; not persisting"
                );
                return;
            }
        };
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

    fn save_probe_result(&self, evidence: &custos_model::Evidence) {
        if let Err(err) = self.block(|pool| {
            let evidence = evidence.clone();
            async move {
                query(
                    "INSERT INTO probe_results                      (resource_id, probe_name, succeeded, observation, collected_at_unix_ms)                      VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(evidence.resource_id.as_str())
                .bind(&evidence.probe_name)
                .bind(evidence.succeeded)
                .bind(&evidence.observation)
                .bind(evidence.collected_at_unix_ms as i64)
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", resource = evidence.resource_id.as_str(), error = %err, "save_probe_result failed");
        }
    }

    fn save_diagnosis(
        &self,
        run_id: Option<&RecoveryRunId>,
        diagnosis: &custos_model::Diagnosis,
        at_unix_ms: u64,
    ) {
        let evidence_refs = serde_json::to_string(&diagnosis.evidence_refs).unwrap_or_default();
        if let Err(err) = self.block(|pool| {
            let run_id = run_id.cloned();
            let diagnosis = diagnosis.clone();
            let evidence_refs = evidence_refs.clone();
            async move {
                query(
                    "INSERT INTO diagnoses                      (run_id, resource_id, fault_code, summary, evidence_refs, at_unix_ms)                      VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .bind(run_id.as_ref().map(|id| id.as_str()))
                .bind(
                    diagnosis
                        .affected_resources
                        .first()
                        .map(|id| id.as_str()),
                )
                .bind(enum_key(&diagnosis.fault_code))
                .bind(&diagnosis.summary)
                .bind(&evidence_refs)
                .bind(at_unix_ms as i64)
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", error = %err, "save_diagnosis failed");
        }
    }

    fn save_incident(&self, incident: &custos_model::Incident) {
        if let Err(err) = self.block(|pool| {
            let incident = incident.clone();
            async move {
                query(
                    "INSERT INTO incidents                      (id, resource_id, fault_code, summary, opened_at_unix_ms, closed_at_unix_ms)                      VALUES (?1, ?2, ?3, ?4, ?5, ?6)                      ON CONFLICT(id) DO UPDATE SET closed_at_unix_ms=?6",
                )
                .bind(incident.id.as_str())
                .bind(incident.resource_id.as_str())
                .bind(enum_key(&incident.fault_code))
                .bind(&incident.summary)
                .bind(incident.opened_at_unix_ms as i64)
                .bind(incident.closed_at_unix_ms.map(|t| t as i64))
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", incident = incident.id.as_str(), error = %err, "save_incident failed");
        }
    }

    fn save_step(
        &self,
        run_id: &RecoveryRunId,
        step_no: u32,
        phase: &str,
        detail: &str,
        at_unix_ms: u64,
    ) {
        if let Err(err) = self.block(|pool| {
            let run_id = run_id.clone();
            let phase = phase.to_owned();
            let detail = detail.to_owned();
            async move {
                query(
                    "INSERT INTO recovery_steps                      (run_id, step_no, phase, detail, at_unix_ms)                      VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(run_id.as_str())
                .bind(step_no as i64)
                .bind(&phase)
                .bind(&detail)
                .bind(at_unix_ms as i64)
                .execute(&pool)
                .await
            }
        }) {
            tracing::error!(target: "custos::store", run = run_id.as_str(), error = %err, "save_step failed");
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

fn status_key(status: &custos_model::RecoveryRunStatus) -> String {
    serde_json::to_string(status)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}

fn enum_key<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value)
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

//! Engine concern: recovery planning, attempt execution, budget gating and run finish.

use super::*;

impl Engine {
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
    /// The full recovery cycle for one resource: open the run, diagnose,
    /// then hand over to [`Engine::drive`].
    pub(super) fn recover(&mut self, id: &ResourceId) -> RecoveryRun {
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
        self.emit_notification(
            custos_model::EventType::RecoveryStarted,
            custos_model::Severity::Info,
            format!(
                "recovery started: {}",
                run.diagnosis
                    .as_ref()
                    .map(|d| d.summary.as_str())
                    .unwrap_or("failure threshold reached")
            ),
            id,
            Some(&run.id),
            run.incident_id.as_ref(),
        );

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
    pub(super) fn drive(
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
    pub(super) fn budget_gate(
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
    pub(super) fn execute_attempt(
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
        // §7.3: the attempt is persisted as `Executing` BEFORE the external
        // action runs. The in-memory run only gets the attempt once its
        // outcome is known — pushing the `outcome: None` skeleton here and
        // never updating it left every in-memory attempt outcome blank.
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
        self.emit_notification(
            custos_model::EventType::RecoveryActionStarted,
            custos_model::Severity::Info,
            format!("plan `{}` attempt {attempt_no} started", plan.plan_id),
            id,
            Some(&run.id),
            run.incident_id.as_ref(),
        );

        // Watchdog-bounded (C20): the executor's own `timeout` is a promise,
        // not a guarantee — a hung executor must not take the tick thread
        // down with it. Past the deadline the attempt records
        // OutcomeUnknown (§6.6).
        let outcome = crate::executor::execute_with_watchdog(
            std::sync::Arc::clone(&self.executor),
            &plan.action,
            Duration::from_millis(plan.timeout_ms),
        );
        let now = self.clock.now_unix_ms();
        attempt.outcome = Some(outcome);
        attempt.finished_at_unix_ms = Some(now);
        self.store.save_attempt(&run.id, &attempt);
        run.action_attempts.push(attempt);
        self.budget_state.record_attempt(id, now, &self.budget);
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
        // A failed action is a notification event; a successful one is not
        // (§8.4 has no recovery.action_succeeded — the verified recovery
        // itself is the success signal).
        if outcome == ActionOutcome::Failed {
            self.emit_notification(
                custos_model::EventType::RecoveryActionFailed,
                custos_model::Severity::Warning,
                format!("plan `{}` attempt {attempt_no} failed", plan.plan_id),
                id,
                Some(&run.id),
                run.incident_id.as_ref(),
            );
        }
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
    pub(super) fn finish_run(&mut self, run: &mut RecoveryRun, event: EngineEvent) {
        self.store.save_run(run);
        // Terminal outcomes become notification events (§8.4).
        let notification = match &event {
            EngineEvent::RecoverySucceeded { .. } => Some((
                custos_model::EventType::RecoverySucceeded,
                custos_model::Severity::Info,
                "recovery verified, resource healthy".to_owned(),
            )),
            EngineEvent::RecoveryExhausted { .. } => Some((
                custos_model::EventType::RecoveryExhausted,
                custos_model::Severity::Critical,
                "all recovery plans exhausted, escalation required".to_owned(),
            )),
            _ => None,
        };
        if let Some((event_type, severity, summary)) = notification {
            let id = run.resource_id.clone();
            self.emit_notification(
                event_type,
                severity,
                summary,
                &id,
                Some(&run.id),
                run.incident_id.as_ref(),
            );
        }
        self.store.emit(event);
    }
    /// Closes the incident of a verified-recovered run; escalated runs keep
    /// their incident open for the human (§6.3).
    pub(super) fn close_incident_of(&mut self, run: &RecoveryRun) {
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
    pub(super) fn finish_escalated(&mut self, run: &mut RecoveryRun, id: &ResourceId) {
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
    pub(super) fn apply_cooldown(&mut self, id: &ResourceId) {
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
}

//! Engine concern: dependency and alternative-group satisfaction, run suspension.

use super::*;

impl Engine {
    /// §3.6 dependency resolution: re-probe binding dependencies, recover
    /// unhealthy ones (recursively), then re-verify — never assume success.
    ///
    /// Returns false when the run must stop driving. That is either an
    /// escalation or a suspension: when the dependency is already being
    /// recovered (or is cooling down towards its next attempt), the run
    /// waits on that SAME result instead of escalating alongside it — all
    /// dependents of one dependency share a single recovery (dedup).
    pub(super) fn dependencies_satisfied(
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
            if dep.requirement == custos_model::DependencyRequirement::AlternativeGroup {
                if !self.alternative_group_satisfied(run, id, dep, step) {
                    return false;
                }
                continue;
            }
            let target = dep.target_resource_id.clone();
            if !self.single_dependency_satisfied(id, run, &target, step) {
                return false;
            }
        }
        true
    }
    /// §3.6 for one binding single-resource dependency: re-probe, recover if
    /// unhealthy, re-verify. Returns false when the run must stop driving.
    pub(super) fn single_dependency_satisfied(
        &mut self,
        id: &ResourceId,
        run: &mut RecoveryRun,
        target: &ResourceId,
        step: &mut u32,
    ) -> bool {
        // Re-probe: state must be fresh, not assumed (§2.3 rules 4/5).
        self.probe_one(target, self.clock.now_unix_ms());
        if self.managed[target].snapshot.status != HealthStatus::Healthy {
            // Recover the dependency first (its own policy and budgets),
            // unless a recovery is already in flight or its next attempt
            // is scheduled behind a cooldown — then wait on that result.
            if self.active_runs.contains(target)
                || now_lt(
                    self.clock.now_unix_ms(),
                    self.managed[target].cooldown_until_ms,
                )
            {
                self.suspend_on(run, target, step);
                return false;
            }
            let _ = self.try_recover(target);
            // Re-verify after dependency recovery.
            self.probe_one(target, self.clock.now_unix_ms());
            if self.managed[target].snapshot.status != HealthStatus::Healthy {
                if now_lt(
                    self.clock.now_unix_ms(),
                    self.managed[target].cooldown_until_ms,
                ) {
                    // The dependency just failed an attempt and is cooling
                    // down towards its next one: wait for that result.
                    self.suspend_on(run, target, step);
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
        true
    }
    /// §3.3/§3.6 N-of-M backup group: satisfied when at least
    /// `required_count` members are healthy. Unhealthy members are recovered
    /// first (each with its own policy/budget), then the group is re-judged;
    /// if it still cannot satisfy N the run escalates.
    pub(super) fn alternative_group_satisfied(
        &mut self,
        run: &mut RecoveryRun,
        id: &ResourceId,
        dep: &custos_model::Dependency,
        step: &mut u32,
    ) -> bool {
        let group_id = dep.alternative_group_id.as_deref();
        let Some(group) = group_id.and_then(|gid| {
            self.alternative_groups
                .iter()
                .find(|g| g.group_id == gid)
                .cloned()
        }) else {
            self.finish_escalated(run, id);
            return false;
        };
        let members = group.members.clone();
        let required = group.required_count;
        // Re-probe every member: state must be fresh, never assumed.
        for member in &members {
            self.probe_one(member, self.clock.now_unix_ms());
        }
        // A group member that is not a managed resource can never be
        // satisfied (and offers no recovery); indexing would panic, so a
        // missing member counts as unhealthy and is skipped below.
        if group.satisfied_by(&|m| {
            self.managed
                .get(m)
                .is_some_and(|managed| managed.snapshot.status == HealthStatus::Healthy)
        }) {
            return true;
        }
        // Recover each unhealthy member (respecting in-flight/cooldown), then
        // re-probe and re-judge the group.
        for member in &members {
            let Some(managed) = self.managed.get(member) else {
                continue;
            };
            if managed.snapshot.status == HealthStatus::Healthy {
                continue;
            }
            if self.active_runs.contains(member)
                || now_lt(self.clock.now_unix_ms(), managed.cooldown_until_ms)
            {
                continue;
            }
            let _ = self.try_recover(member);
            self.probe_one(member, self.clock.now_unix_ms());
        }
        if group.satisfied_by(&|m| {
            self.managed
                .get(m)
                .is_some_and(|managed| managed.snapshot.status == HealthStatus::Healthy)
        }) {
            return true;
        }
        self.store.emit(EngineEvent::DependencyUnsatisfied {
            run_id: run.id.clone(),
            resource_id: id.clone(),
            dependency_id: group_id
                .map(|g| ResourceId::new(format!("group:{g}")))
                .unwrap_or_else(|| ResourceId::new("group")),
            at_unix_ms: self.clock.now_unix_ms(),
        });
        self.store.save_step(
            &run.id,
            bump(step),
            "resolving_dependencies",
            &format!(
                "alternative group `{}` unsatisfied ({required} of {} required)",
                group.group_id,
                members.len()
            ),
            self.clock.now_unix_ms(),
        );
        tracing::warn!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = id.as_str(),
            group = group.group_id.as_str(),
            "alternative group unsatisfied after recovery: upstream action blocked"
        );
        self.finish_escalated(run, id);
        false
    }
    /// Suspends the run in `Waiting` on `target`, registering it as a
    /// waiter (§3.6 dedup). On wait-budget exhaustion the run escalates
    /// instead. Either way driving stops for this tick.
    pub(super) fn suspend_on(
        &mut self,
        run: &mut RecoveryRun,
        target: &ResourceId,
        step: &mut u32,
    ) {
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
}

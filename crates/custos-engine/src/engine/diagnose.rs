//! Engine concern: fault diagnosis and post-action verification.

use super::*;

impl Engine {
    /// Maps the freshest probe evidence to a fault code (docs §5.1: probes
    /// collect facts, diagnosis judges).
    pub(super) fn diagnose(&mut self, id: &ResourceId, now_unix_ms: u64) -> Diagnosis {
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
    pub(super) fn verify(
        &mut self,
        id: &ResourceId,
        plan: &RecoveryPlan,
        run: &mut RecoveryRun,
    ) -> bool {
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
        tracing::debug!(
            target: "custos::engine",
            run = run.id.as_str(),
            resource = id.as_str(),
            plan = plan.plan_id.as_str(),
            passed,
            "verification checked"
        );
        passed
    }
}

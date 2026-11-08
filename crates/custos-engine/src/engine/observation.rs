//! Engine concern: single-resource probing and evidence persistence.

use super::*;

impl Engine {
    /// Probes one resource and applies jitter-suppressed threshold logic.
    pub fn probe_one(&mut self, id: &ResourceId, now_unix_ms: u64) -> ProbeReport {
        let (report, thresholds) = {
            let managed = &self.managed[id];
            // A managed resource without a wired probe must not take down
            // the whole recovery loop (no startup validation guarantees the
            // wiring): report an honest Unreachable and let policy react.
            let (report, probe_name) = match self.probes.get(id) {
                Some(probe) => (probe.probe(&managed.resource), probe.name().to_owned()),
                None => (
                    ProbeReport {
                        resource_id: id.clone(),
                        probe_name: "unwired".to_owned(),
                        succeeded: false,
                        status: HealthStatus::Unreachable,
                        observation: "no probe is wired for this resource".to_owned(),
                    },
                    "unwired".to_owned(),
                ),
            };
            // Evidence is persisted for every probe (§7.2 probe_results):
            // diagnosis must be re-derivable from stored facts.
            self.store.save_probe_result(&custos_model::Evidence {
                resource_id: id.clone(),
                probe_name,
                succeeded: report.succeeded,
                observation: report.observation.clone(),
                collected_at_unix_ms: now_unix_ms,
            });
            (
                report,
                (
                    managed.resource.health_policy.failure_threshold,
                    managed.resource.health_policy.success_threshold,
                ),
            )
        };
        let before = self.managed[id].snapshot.status;
        let snapshot = &mut self.managed.get_mut(id).unwrap().snapshot;
        snapshot.record_probe(
            report.succeeded,
            report.status,
            thresholds.0,
            thresholds.1,
            now_unix_ms,
        );
        let after = self.managed[id].snapshot.status;
        // Health transitions become notification events (§8.4): a first
        // unhealthy/degraded state is the operator's signal. Unreachable and
        // Recovering have no notification event type by design, and the
        // incident opens later (with the recovery run), so none is attached
        // here.
        if before != after {
            match after {
                HealthStatus::Unhealthy => self.emit_notification(
                    custos_model::EventType::ResourceUnhealthy,
                    custos_model::Severity::Critical,
                    Self::transition_summary("resource is unhealthy", &report.observation),
                    id,
                    None,
                    None,
                ),
                HealthStatus::Degraded => self.emit_notification(
                    custos_model::EventType::ResourceDegraded,
                    custos_model::Severity::Warning,
                    Self::transition_summary("resource is degraded", &report.observation),
                    id,
                    None,
                    None,
                ),
                // A plugin-backed probe that goes unreachable because the
                // plugin supervisor is unrecoverable raises the plugin
                // fault (§4.5 step 9, §8.4) — the resource itself stays
                // Unreachable, not Unhealthy, so recovery is not attempted
                // on a probe that cannot produce evidence.
                HealthStatus::Unreachable
                    if report
                        .observation
                        .starts_with(custos_probe::PLUGIN_DEGRADED_OBSERVATION_PREFIX) =>
                {
                    self.emit_notification(
                        custos_model::EventType::PluginUnavailable,
                        custos_model::Severity::Critical,
                        format!(
                            "probe `{}` reports: {}",
                            report.probe_name, report.observation
                        ),
                        id,
                        None,
                        None,
                    );
                }
                _ => {}
            }
        }
        tracing::debug!(
            target: "custos::engine",
            resource = id.as_str(),
            succeeded = report.succeeded,
            observation = %report.observation,
            "probe collected"
        );
        report
    }
}

//! Plugin-backed probe: lets a resource's probe be served by a plugin
//! process through the same [`PluginSupervisor`] restart policy
//! (docs/guide/plugins.md §4.5: an unrecoverable plugin marks its
//! resources unprobeable).
//!
//! The adapter implements the engine-facing [`Probe`] trait with the
//! status conventions of the builtin probes:
//! * plugin answers `healthy: true` → succeeded, `Healthy`;
//! * plugin answers `healthy: false` → succeeded=false, `Unhealthy`
//!   (the probe worked and reports a problem);
//! * plugin unavailable/degraded/timed out/protocol error → succeeded=false,
//!   `Unreachable` — the probe could not obtain a valid result, which is
//!   exactly the "resource marked unprobeable" rule (§4.5 step 9).
//!
//! A degraded supervisor is distinguished from a crash: the observation
//! names the degradation so the engine's event/notification layer can
//! raise `plugin.unavailable` (§8.4) instead of re-probing forever.

use crate::supervisor::{PluginStatus, PluginSupervisor};
use custos_model::{HealthStatus, Resource};
use custos_plugin_api::{Method, OperationParams};
use custos_probe::{Probe, ProbeReport};
use std::sync::Arc;

/// A probe whose execution is a plugin operation.
pub struct PluginProbe {
    supervisor: Arc<PluginSupervisor>,
    /// Capability operation the plugin provides for probing (§4.6).
    operation_id: String,
    probe_name: &'static str,
}

impl PluginProbe {
    pub fn new(
        supervisor: Arc<PluginSupervisor>,
        operation_id: impl Into<String>,
        probe_name: &'static str,
    ) -> Self {
        Self {
            supervisor,
            operation_id: operation_id.into(),
            probe_name,
        }
    }

    /// The plugin that serves this probe.
    pub fn supervisor(&self) -> &PluginSupervisor {
        &self.supervisor
    }

    /// Maps a call failure onto the probe's "could not obtain a result"
    /// observation, naming degradation explicitly.
    fn unreachable_report(&self, resource: &Resource, error: &crate::HostError) -> ProbeReport {
        let reason = match &self.supervisor.status() {
            PluginStatus::Degraded { reason } => format!("plugin degraded (unprobeable): {reason}"),
            _ => error.to_string(),
        };
        ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.probe_name.to_owned(),
            succeeded: false,
            status: HealthStatus::Unreachable,
            observation: reason,
        }
    }
}

impl Probe for PluginProbe {
    fn name(&self) -> &'static str {
        self.probe_name
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let params = OperationParams {
            operation_id: self.operation_id.clone(),
            resource_id: resource.id.to_string(),
            run_id: None,
            args: Default::default(),
        };
        let params = serde_json::to_value(&params).unwrap_or_default();
        match self.supervisor.call(Method::Probe, params) {
            Ok(result) => {
                let healthy = result
                    .get("healthy")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let observation = result
                    .get("observation")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned();
                if healthy {
                    ProbeReport {
                        resource_id: resource.id.clone(),
                        probe_name: self.probe_name.to_owned(),
                        succeeded: true,
                        status: HealthStatus::Healthy,
                        observation,
                    }
                } else {
                    ProbeReport {
                        resource_id: resource.id.clone(),
                        probe_name: self.probe_name.to_owned(),
                        succeeded: false,
                        status: HealthStatus::Unhealthy,
                        observation,
                    }
                }
            }
            Err(error) => self.unreachable_report(resource, &error),
        }
    }
}

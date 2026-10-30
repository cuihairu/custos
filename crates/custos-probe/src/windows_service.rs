//! Windows SCM service state probe (survey §26.11, implementation order #11)
//! — the native Windows implementation of the `service` resource kind,
//! mirroring how `systemd.rs` is the Linux implementation.
//!
//! Uses the mature `windows-service` crate: open the SCM, open the named
//! service with QUERY_STATUS | QUERY_CONFIG, then read its current state
//! (QueryServiceStatus) and start type (QueryServiceConfig). Normalization
//! follows the survey: `StartType=auto` and state ≠ running → Unhealthy; the
//! transitional states → Degraded; a service that does not exist is a
//! definite Unhealthy (the monitored service is not running). The status
//! mapping is a portable `classify()` (unit-tested everywhere); only the SCM
//! call itself is `cfg(windows)`. Non-Windows targets report Unreachable.

use crate::config::service_name;
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};

/// Probes one Windows SCM service.
#[derive(Debug, Clone)]
pub struct WindowsServiceProbe {
    pub name: String,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.11).
pub fn from_config(config: &serde_json::Value) -> Result<WindowsServiceProbe, String> {
    let name = service_name(config, "unit")
        .ok_or("`probe.unit` is required for Windows service probes")?;
    Ok(WindowsServiceProbe { name })
}

impl Probe for WindowsServiceProbe {
    fn name(&self) -> &'static str {
        "windows-service"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        #[cfg(windows)]
        {
            self.query_and_report(&report)
        }
        #[cfg(not(windows))]
        {
            let _ = &report;
            report(
                false,
                HealthStatus::Unreachable,
                "Windows services are only available on Windows".to_owned(),
            )
        }
    }
}

/// A service's observable state, decoupled from the SCM enum so the
/// normalization is unit-testable on every platform. Only compiled where it
/// is used (Windows SCM mapping, or the platform-independent unit tests).
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceSignal {
    Running,
    Starting,
    Stopping,
    Paused,
    Stopped,
}

/// A service's start disposition, decoupled from `ServiceStartType`.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartKind {
    Auto,
    Manual,
}

/// Survey rule: `StartType=auto` and state ≠ running → Unhealthy; a
/// manually-started service that is stopped is expected and Healthy;
/// transitional states are Degraded (the service is still doing something).
#[cfg(any(windows, test))]
fn classify(signal: ServiceSignal, start: StartKind) -> (bool, HealthStatus, String) {
    match signal {
        ServiceSignal::Running => (true, HealthStatus::Healthy, "running".to_owned()),
        ServiceSignal::Starting => (false, HealthStatus::Degraded, "starting".to_owned()),
        ServiceSignal::Stopping => (false, HealthStatus::Degraded, "stopping".to_owned()),
        ServiceSignal::Paused => (false, HealthStatus::Degraded, "paused".to_owned()),
        ServiceSignal::Stopped => {
            if start == StartKind::Auto {
                (
                    false,
                    HealthStatus::Unhealthy,
                    "stopped (auto-start, should be running)".to_owned(),
                )
            } else {
                (
                    true,
                    HealthStatus::Healthy,
                    "stopped (expected, manual start)".to_owned(),
                )
            }
        }
    }
}

#[cfg(windows)]
enum QueryOutcome {
    Found(ServiceSignal, StartKind),
    NotFound,
}

#[cfg(windows)]
impl WindowsServiceProbe {
    fn query_and_report(
        &self,
        report: &impl Fn(bool, HealthStatus, String) -> ProbeReport,
    ) -> ProbeReport {
        match query_service_state(&self.name) {
            Ok(QueryOutcome::Found(signal, start)) => {
                let (succeeded, status, observation) = classify(signal, start);
                report(succeeded, status, observation)
            }
            Ok(QueryOutcome::NotFound) => report(
                false,
                HealthStatus::Unhealthy,
                format!("service {} does not exist", self.name),
            ),
            Err(err) => report(
                false,
                HealthStatus::Unreachable,
                format!("cannot query service {}: {err}", self.name),
            ),
        }
    }
}

/// `ERROR_SERVICE_DOES_NOT_EXIST` — the service name is not registered.
#[cfg(windows)]
const SERVICE_DOES_NOT_EXIST: i32 = 1060;

#[cfg(windows)]
use windows_service::service::{ServiceAccess, ServiceStartType, ServiceState};

#[cfg(windows)]
fn query_service_state(name: &str) -> Result<QueryOutcome, String> {
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager =
        ServiceManager::local_computer(None::<&std::ffi::OsStr>, ServiceManagerAccess::CONNECT)
            .map_err(|err| format!("open service manager: {err}"))?;
    let service = match manager.open_service(
        name,
        ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
    ) {
        Ok(service) => service,
        Err(windows_service::Error::Winapi(io))
            if io.raw_os_error() == Some(SERVICE_DOES_NOT_EXIST) =>
        {
            return Ok(QueryOutcome::NotFound);
        }
        Err(err) => return Err(format!("open service: {err}")),
    };
    let status = service
        .query_status()
        .map_err(|err| format!("query status: {err}"))?;
    let config = service
        .query_config()
        .map_err(|err| format!("query config: {err}"))?;
    Ok(QueryOutcome::Found(
        to_signal(status.current_state),
        to_start(config.start_type),
    ))
}

#[cfg(windows)]
fn to_signal(state: ServiceState) -> ServiceSignal {
    match state {
        ServiceState::Running => ServiceSignal::Running,
        ServiceState::StartPending | ServiceState::ContinuePending => ServiceSignal::Starting,
        ServiceState::StopPending | ServiceState::PausePending => ServiceSignal::Stopping,
        ServiceState::Paused => ServiceSignal::Paused,
        ServiceState::Stopped => ServiceSignal::Stopped,
    }
}

#[cfg(windows)]
fn to_start(start_type: ServiceStartType) -> StartKind {
    match start_type {
        // The driver start types and the disabled/demand ones are all
        // "expected to be stopped" until something starts them.
        ServiceStartType::AutoStart => StartKind::Auto,
        _ => StartKind::Manual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_config_requires_unit() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"unit": "  "})).is_err());
        assert!(from_config(&json!({"unit": "a b"})).is_err());
        assert!(from_config(&json!({"unit": "spooler"})).is_ok());
    }

    #[test]
    fn running_is_healthy() {
        let (ok, status, obs) = classify(ServiceSignal::Running, StartKind::Auto);
        assert!(ok);
        assert_eq!(status, HealthStatus::Healthy);
        assert_eq!(obs, "running");
    }

    #[test]
    fn auto_start_stopped_is_unhealthy() {
        let (ok, status, _) = classify(ServiceSignal::Stopped, StartKind::Auto);
        assert!(!ok);
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[test]
    fn manual_start_stopped_is_healthy() {
        let (ok, status, obs) = classify(ServiceSignal::Stopped, StartKind::Manual);
        assert!(ok);
        assert_eq!(status, HealthStatus::Healthy);
        assert!(obs.contains("expected"));
    }

    #[test]
    fn transitional_states_are_degraded() {
        for (signal, label) in [
            (ServiceSignal::Starting, "starting"),
            (ServiceSignal::Stopping, "stopping"),
            (ServiceSignal::Paused, "paused"),
        ] {
            let (ok, status, obs) = classify(signal, StartKind::Auto);
            assert!(!ok);
            assert_eq!(status, HealthStatus::Degraded, "{label}");
            assert_eq!(obs, label);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_reports_unreachable() {
        let probe = WindowsServiceProbe {
            name: "spooler".into(),
        };
        let resource = Resource::new("svc", "service");
        let report = probe.probe(&resource);
        assert_eq!(report.status, HealthStatus::Unreachable);
        assert!(report.observation.contains("only available on Windows"));
    }
}

//! Resource health status (docs/guide/resources.md §3.2).

use serde::{Deserialize, Serialize};

/// Health status of a resource. `Unreachable` must not be treated as
/// equivalent to `Unhealthy`: a probe that cannot obtain a valid result does
/// not yet confirm the root cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Not enough evidence collected yet.
    Unknown,
    /// Health criteria satisfied.
    Healthy,
    /// Some capability impaired, but the service still works.
    Degraded,
    /// Health criteria clearly not satisfied.
    Unhealthy,
    /// A recovery process is being executed.
    Recovering,
    /// Automatic handling paused by policy.
    Suppressed,
    /// The probe itself cannot obtain a valid result.
    Unreachable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_snake_case() {
        let json = serde_json::to_string(&HealthStatus::Unreachable).unwrap();
        assert_eq!(json, "\"unreachable\"");
        let back: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HealthStatus::Unreachable);
    }
}

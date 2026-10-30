//! Shared threshold model for the system-metrics probes (survey §26.5,
//! implementation order #5 — the disk/memory/CPU trio shares one model).
//!
//! `PctThreshold` is a warn/crit percentage pair; `LoadThreshold` is a
//! warn/crit load-per-core ratio pair (a load average of 1.0 per core is
//! a fully-loaded system). Both classify to the 7-state model: at/above
//! crit → Unhealthy, at/above warn → Degraded, below → Healthy.

use crate::config::u64_or;
use custos_model::HealthStatus;
use serde_json::Value;

/// Percentage usage threshold pair (warn < crit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PctThreshold {
    pub warn_pct: u64,
    pub crit_pct: u64,
}

impl PctThreshold {
    pub fn classify(&self, pct: u64) -> HealthStatus {
        if pct >= self.crit_pct {
            HealthStatus::Unhealthy
        } else if pct >= self.warn_pct {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }
}

/// Load-average-per-core threshold pair (warn < crit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadThreshold {
    pub warn_per_core: f64,
    pub crit_per_core: f64,
}

impl LoadThreshold {
    pub fn classify(&self, load_per_core: f64) -> HealthStatus {
        if load_per_core >= self.crit_per_core {
            HealthStatus::Unhealthy
        } else if load_per_core >= self.warn_per_core {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }
}

/// Reads a `warn_pct`/`crit_pct` pair (defaults 80/90), validating the
/// ordering and range.
pub(crate) fn pct_threshold(config: &Value, what: &str) -> Result<PctThreshold, String> {
    let warn_pct = u64_or(config, "warn_pct", 80);
    let crit_pct = u64_or(config, "crit_pct", 90);
    validate_pct(warn_pct, crit_pct, what)?;
    Ok(PctThreshold { warn_pct, crit_pct })
}

fn validate_pct(warn_pct: u64, crit_pct: u64, what: &str) -> Result<(), String> {
    if warn_pct == 0 || warn_pct > 100 {
        return Err(format!("`probe.warn_pct` for {what} must be in 1..=100"));
    }
    if crit_pct > 100 {
        return Err(format!("`probe.crit_pct` for {what} must be in 1..=100"));
    }
    if crit_pct < warn_pct {
        return Err(format!(
            "`probe.crit_pct` ({crit_pct}) must be >= `probe.warn_pct` ({warn_pct}) for {what}"
        ));
    }
    Ok(())
}

/// Reads a `warn_load`/`crit_load` per-core pair (defaults 0.7/1.0).
pub(crate) fn load_threshold(config: &Value) -> Result<LoadThreshold, String> {
    let warn_per_core = config["warn_load"].as_f64().unwrap_or(0.7);
    let crit_per_core = config["crit_load"].as_f64().unwrap_or(1.0);
    if !warn_per_core.is_finite() || warn_per_core <= 0.0 {
        return Err("`probe.warn_load` must be a positive number".to_owned());
    }
    if !crit_per_core.is_finite() || crit_per_core <= 0.0 {
        return Err("`probe.crit_load` must be a positive number".to_owned());
    }
    if crit_per_core < warn_per_core {
        return Err(format!(
            "`probe.crit_load` ({crit_per_core}) must be >= `probe.warn_load` ({warn_per_core})"
        ));
    }
    Ok(LoadThreshold {
        warn_per_core,
        crit_per_core,
    })
}

/// Integer percentage of `used` over `total`, saturating (never > 100),
/// guarding against a zero denominator.
pub(crate) fn percent(used: u64, total: u64) -> u64 {
    if total == 0 {
        return 100;
    }
    (used.saturating_mul(100).min(total.saturating_mul(100))) / total
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pct_threshold_classifies() {
        let t = PctThreshold {
            warn_pct: 80,
            crit_pct: 90,
        };
        assert_eq!(t.classify(0), HealthStatus::Healthy);
        assert_eq!(t.classify(79), HealthStatus::Healthy);
        assert_eq!(t.classify(80), HealthStatus::Degraded);
        assert_eq!(t.classify(89), HealthStatus::Degraded);
        assert_eq!(t.classify(90), HealthStatus::Unhealthy);
        assert_eq!(t.classify(100), HealthStatus::Unhealthy);
    }

    #[test]
    fn load_threshold_classifies() {
        let t = LoadThreshold {
            warn_per_core: 0.7,
            crit_per_core: 1.0,
        };
        assert_eq!(t.classify(0.5), HealthStatus::Healthy);
        assert_eq!(t.classify(0.7), HealthStatus::Degraded);
        assert_eq!(t.classify(0.99), HealthStatus::Degraded);
        assert_eq!(t.classify(1.0), HealthStatus::Unhealthy);
        assert_eq!(t.classify(4.0), HealthStatus::Unhealthy);
    }

    #[test]
    fn pct_config_validates_order_and_range() {
        assert!(pct_threshold(&json!({"warn_pct": 90, "crit_pct": 80}), "disk").is_err());
        assert!(pct_threshold(&json!({"warn_pct": 0}), "disk").is_err());
        assert!(pct_threshold(&json!({"crit_pct": 101}), "disk").is_err());
        let ok = pct_threshold(&json!({"warn_pct": 70, "crit_pct": 85}), "disk").unwrap();
        assert_eq!(ok.warn_pct, 70);
        assert_eq!(ok.crit_pct, 85);
    }

    #[test]
    fn load_config_validates() {
        assert!(load_threshold(&json!({"warn_load": 1.0, "crit_load": 0.5})).is_err());
        assert!(load_threshold(&json!({"warn_load": -1.0})).is_err());
        let ok = load_threshold(&json!({"warn_load": 0.5, "crit_load": 0.8})).unwrap();
        assert_eq!(ok.warn_per_core, 0.5);
    }

    #[test]
    fn percent_saturates_and_guards_zero() {
        assert_eq!(percent(50, 100), 50);
        assert_eq!(percent(100, 100), 100);
        assert_eq!(percent(150, 100), 100);
        assert_eq!(percent(0, 0), 100);
    }
}

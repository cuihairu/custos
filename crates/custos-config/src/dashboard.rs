//! Optional web dashboard configuration (docs/guide/dashboard-cli-config.md
//! §9.1): an opt-in, read-only surface. Disabled by default — when `enabled`
//! is false the runtime binds no listener and no HTTP code path runs, so the
//! core engine is never affected by the dashboard being absent.

use serde::Deserialize;
use std::net::SocketAddr;

/// Validated `[dashboard]` section carried into the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardConfig {
    pub enabled: bool,
    pub listen: ListenConfig,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: ListenConfig {
                addr: SocketAddr::new([127, 0, 0, 1].into(), 8080),
            },
        }
    }
}

/// The bind address/port for the dashboard listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenConfig {
    pub addr: SocketAddr,
}

impl DashboardConfig {
    /// True when the dashboard should serve; false means no listener.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The listener address.
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen.addr
    }
}

/// Raw `[dashboard]` as written in the TOML.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDashboard {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_listen")]
    listen: String,
}

fn default_listen() -> String {
    "127.0.0.1:8080".to_owned()
}

/// Parses the `[dashboard]` section; a missing section means disabled.
pub(crate) fn parse(raw: Option<RawDashboard>) -> Result<DashboardConfig, crate::ConfigError> {
    let raw = match raw {
        Some(raw) => raw,
        None => return Ok(DashboardConfig::default()),
    };
    let addr: SocketAddr = raw.listen.parse().map_err(|e| {
        crate::ConfigError::validation("dashboard.listen", format!("invalid listen address: {e}"))
    })?;
    Ok(DashboardConfig {
        enabled: raw.enabled,
        listen: ListenConfig { addr },
    })
}

//! Configuration parsing and validation (docs/guide/dashboard-cli-config.md
//! §10, docs/guide/resources.md §3.5).
//!
//! Configuration load time is the gate: unknown fields are errors, duplicate
//! IDs are errors, dangling dependency references are errors, impossible
//! dependency conditions are errors, and dependency cycles are errors —
//! startup is rejected instead of recursing forever during recovery.
//! Every validation failure reports the concrete field path (§10.3).

mod notification;
pub use notification::{
    ChannelConfig, HeraldAuth, HeraldConfig, NotificationConfig, NotificationRule, SecretString,
    SmtpConfig, SsrfPolicy, TelegramConfig, WebhookConfig,
};

mod dashboard;
pub use dashboard::{DashboardConfig, ListenConfig};

/// Supported configuration schema version.
pub const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported configuration version {found} at `version` (supported: {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("{path}: {message}")]
    Validation { path: String, message: String },
}

impl ConfigError {
    fn validation(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            path: path.into(),
            message: message.into(),
        }
    }
}

mod build;
mod schema;

pub use build::{load, Permissions, ValidatedConfig};

#[cfg(test)]
mod tests;

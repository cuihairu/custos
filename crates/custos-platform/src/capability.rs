//! Capability description for platform adapters (§11.1: 平台专属能力通过
//! Capability 描述，不得伪造平台支持).
//!
//! An adapter answers "which operations do you serve, and how" — the answer
//! is data, so the runtime (toolbox registry, docs) can report support
//! honestly: anything absent from the set is [`PlatformError::Unsupported`],
//! never silently mapped to a similar-looking operation.

use crate::error::{PlatformError, PlatformResult};
use std::collections::BTreeMap;

/// How an adapter serves one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// Served natively by this adapter.
    Native,
    /// Not served here; the string names the concrete reason (e.g. "this
    /// host has no systemd").
    Unsupported(String),
}

/// One adapter's declared operation support: operation id → support.
#[derive(Debug, Clone, Default)]
pub struct PlatformCapabilitySet {
    operations: BTreeMap<String, Support>,
}

impl PlatformCapabilitySet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a natively served operation.
    pub fn native(mut self, operation: impl Into<String>) -> Self {
        self.operations.insert(operation.into(), Support::Native);
        self
    }

    /// Declares an operation as unsupported with the concrete reason.
    pub fn unsupported(mut self, operation: impl Into<String>, reason: impl Into<String>) -> Self {
        self.operations
            .insert(operation.into(), Support::Unsupported(reason.into()));
        self
    }

    /// True when the operation is served natively.
    pub fn is_native(&self, operation: &str) -> bool {
        matches!(self.operations.get(operation), Some(Support::Native))
    }

    /// Guards an operation call: native → Ok, absent/unsupported → the
    /// concrete Unsupported error. The adapter MUST call this before doing
    /// anything (绝不误执行).
    pub fn require(&self, operation: &str) -> PlatformResult<()> {
        match self.operations.get(operation) {
            Some(Support::Native) => Ok(()),
            Some(Support::Unsupported(reason)) => {
                Err(PlatformError::Unsupported(format!("{operation}: {reason}")))
            }
            None => Err(PlatformError::Unsupported(format!(
                "{operation}: not provided by this adapter"
            ))),
        }
    }

    /// All declared operations, for reports and docs.
    pub fn operations(&self) -> impl Iterator<Item = (&String, &Support)> {
        self.operations.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> PlatformCapabilitySet {
        PlatformCapabilitySet::new()
            .native("service.status")
            .native("service.restart")
            .unsupported("wifi.reconnect", "no wireless adapter on this host")
    }

    #[test]
    fn native_operations_pass_the_gate() {
        assert!(set().require("service.status").is_ok());
    }

    #[test]
    fn unsupported_operations_carry_the_reason() {
        let err = set().require("wifi.reconnect").unwrap_err();
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("no wireless"), "{err}");
    }

    #[test]
    fn undeclared_operations_are_unsupported_not_guessed() {
        let err = set().require("dns.flush").unwrap_err();
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("not provided"), "{err}");
    }
}

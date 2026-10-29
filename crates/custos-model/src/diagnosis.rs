//! Diagnosis derived from evidence (docs/guide/probe-diagnosis.md §5.1).

use crate::ids::ResourceId;
use serde::{Deserialize, Serialize};

/// Fault classification (docs/guide/probe-diagnosis.md §5.2). Unknown
/// failures must not directly trigger every possible recovery action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultCode {
    ProcessExited,
    ServiceStopped,
    ProbeTimeout,
    NetworkDisconnected,
    GatewayUnreachable,
    DnsFailure,
    ProxyConnectionFailed,
    ProxyAuthenticationFailed,
    RemoteEndpointUnreachable,
    PluginUnavailable,
    DependencyUnsatisfied,
    RecoveryActionFailed,
    VerificationFailed,
    UnknownFailure,
}

/// A fault judgment derived from evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnosis {
    pub fault_code: FaultCode,
    pub summary: String,
    /// Optional rule-based confidence. Must not pretend to be a statistical
    /// probability.
    pub confidence: Option<f32>,
    pub affected_resources: Vec<ResourceId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_code_serializes_as_snake_case() {
        let json = serde_json::to_string(&FaultCode::ProxyAuthenticationFailed).unwrap();
        assert_eq!(json, "\"proxy_authentication_failed\"");
    }

    #[test]
    fn diagnosis_roundtrips() {
        let diagnosis = Diagnosis {
            fault_code: FaultCode::ProxyConnectionFailed,
            summary: "local port open but auth rejected".into(),
            confidence: Some(0.8),
            affected_resources: vec![ResourceId::new("proxy.egress")],
        };
        let back: Diagnosis =
            serde_json::from_str(&serde_json::to_string(&diagnosis).unwrap()).unwrap();
        assert_eq!(back, diagnosis);
    }
}

//! Notification events (docs/guide/notification.md §8.4) — the structured
//! events Core Engine produces and the notification subsystem consumes and
//! persists. They live in the domain model so the producer (custos-engine),
//! the consumer (custos-notify) and the queue persistence (custos-store)
//! share one shape without extra cross-crate dependencies, exactly like
//! `Evidence`, `Diagnosis` and `Incident`.

use crate::ids::{EventId, IncidentId, RecoveryRunId, ResourceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Event severity, ordered by urgency. The three levels map to the
/// notification policy examples (§8.5): log-only record (info), alert
/// (warning), high-priority alert (critical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }

    pub fn from_wire(name: &str) -> Option<Self> {
        match name {
            "info" => Some(Severity::Info),
            "warning" => Some(Severity::Warning),
            "critical" => Some(Severity::Critical),
            _ => None,
        }
    }
}

/// The eleven notification event types (docs/guide/notification.md §8.4).
/// Each serializes to its exact dotted wire name, so logs, queue rows and
/// channel payloads stay stable and greppable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "resource.unhealthy")]
    ResourceUnhealthy,
    #[serde(rename = "resource.degraded")]
    ResourceDegraded,
    #[serde(rename = "recovery.started")]
    RecoveryStarted,
    #[serde(rename = "recovery.action_started")]
    RecoveryActionStarted,
    #[serde(rename = "recovery.action_failed")]
    RecoveryActionFailed,
    #[serde(rename = "recovery.succeeded")]
    RecoverySucceeded,
    #[serde(rename = "recovery.exhausted")]
    RecoveryExhausted,
    #[serde(rename = "recovery.outcome_unknown")]
    RecoveryOutcomeUnknown,
    #[serde(rename = "plugin.unavailable")]
    PluginUnavailable,
    #[serde(rename = "notification.delivery_failed")]
    NotificationDeliveryFailed,
    #[serde(rename = "configuration.invalid")]
    ConfigurationInvalid,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::ResourceUnhealthy => "resource.unhealthy",
            EventType::ResourceDegraded => "resource.degraded",
            EventType::RecoveryStarted => "recovery.started",
            EventType::RecoveryActionStarted => "recovery.action_started",
            EventType::RecoveryActionFailed => "recovery.action_failed",
            EventType::RecoverySucceeded => "recovery.succeeded",
            EventType::RecoveryExhausted => "recovery.exhausted",
            EventType::RecoveryOutcomeUnknown => "recovery.outcome_unknown",
            EventType::PluginUnavailable => "plugin.unavailable",
            EventType::NotificationDeliveryFailed => "notification.delivery_failed",
            EventType::ConfigurationInvalid => "configuration.invalid",
        }
    }

    pub fn from_wire(name: &str) -> Option<Self> {
        match name {
            "resource.unhealthy" => Some(EventType::ResourceUnhealthy),
            "resource.degraded" => Some(EventType::ResourceDegraded),
            "recovery.started" => Some(EventType::RecoveryStarted),
            "recovery.action_started" => Some(EventType::RecoveryActionStarted),
            "recovery.action_failed" => Some(EventType::RecoveryActionFailed),
            "recovery.succeeded" => Some(EventType::RecoverySucceeded),
            "recovery.exhausted" => Some(EventType::RecoveryExhausted),
            "recovery.outcome_unknown" => Some(EventType::RecoveryOutcomeUnknown),
            "plugin.unavailable" => Some(EventType::PluginUnavailable),
            "notification.delivery_failed" => Some(EventType::NotificationDeliveryFailed),
            "configuration.invalid" => Some(EventType::ConfigurationInvalid),
            _ => None,
        }
    }
}

/// Flat key-value details that have passed sensitive-field scrubbing
/// (§8.7): tokens, passwords, private keys and full auth headers must never
/// reach logs or the queue. Constructing `SanitizedDetails` is the explicit
/// point where a producer certifies the contents are safe; the scrubbing
/// itself is a separate concern (todo.md: sensitive-info governance).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SanitizedDetails(BTreeMap<String, String>);

impl SanitizedDetails {
    pub fn new(entries: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> Self {
        Self(
            entries
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter()
    }
}

/// One structured notification event (§8.4). `notification.delivery_failed`
/// events must be special-cased by the notification engine to avoid an
/// infinite self-amplification loop (§8.4, §8.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationEvent {
    pub event_id: EventId,
    pub event_type: EventType,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<ResourceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<IncidentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_run_id: Option<RecoveryRunId>,
    pub occurred_at_unix_ms: u64,
    pub summary: String,
    pub details: SanitizedDetails,
}

impl NotificationEvent {
    /// Builds an event with the optional correlation IDs empty; the builder
    /// methods attach them when the producer knows them.
    pub fn new(
        event_id: EventId,
        event_type: EventType,
        severity: Severity,
        summary: impl Into<String>,
        at_unix_ms: u64,
    ) -> Self {
        Self {
            event_id,
            event_type,
            severity,
            resource_id: None,
            incident_id: None,
            recovery_run_id: None,
            occurred_at_unix_ms: at_unix_ms,
            summary: summary.into(),
            details: SanitizedDetails::default(),
        }
    }

    pub fn with_resource_id(mut self, id: ResourceId) -> Self {
        self.resource_id = Some(id);
        self
    }

    pub fn with_incident_id(mut self, id: IncidentId) -> Self {
        self.incident_id = Some(id);
        self
    }

    pub fn with_recovery_run_id(mut self, id: RecoveryRunId) -> Self {
        self.recovery_run_id = Some(id);
        self
    }

    pub fn with_details(mut self, details: SanitizedDetails) -> Self {
        self.details = details;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_serialize_to_dotted_wire_names() {
        let cases = [
            (EventType::ResourceUnhealthy, "resource.unhealthy"),
            (EventType::ResourceDegraded, "resource.degraded"),
            (EventType::RecoveryStarted, "recovery.started"),
            (EventType::RecoveryActionStarted, "recovery.action_started"),
            (EventType::RecoveryActionFailed, "recovery.action_failed"),
            (EventType::RecoverySucceeded, "recovery.succeeded"),
            (EventType::RecoveryExhausted, "recovery.exhausted"),
            (
                EventType::RecoveryOutcomeUnknown,
                "recovery.outcome_unknown",
            ),
            (EventType::PluginUnavailable, "plugin.unavailable"),
            (
                EventType::NotificationDeliveryFailed,
                "notification.delivery_failed",
            ),
            (EventType::ConfigurationInvalid, "configuration.invalid"),
        ];
        assert_eq!(cases.len(), 11, "exactly eleven event types");
        for (variant, wire) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(variant.as_str(), wire);
            let back: EventType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn severity_serializes_as_snake_case() {
        assert_eq!(serde_json::to_string(&Severity::Info).unwrap(), "\"info\"");
        assert_eq!(
            serde_json::to_string(&Severity::Critical).unwrap(),
            "\"critical\""
        );
        assert_eq!(Severity::Info.as_str(), "info");
    }

    #[test]
    fn severity_orders_by_urgency() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Critical);
    }

    #[test]
    fn event_roundtrips_through_serde() {
        let event = NotificationEvent::new(
            EventId::new("evt-1"),
            EventType::RecoveryExhausted,
            Severity::Critical,
            "all recovery attempts exhausted",
            1_700_000_000_000,
        )
        .with_resource_id(ResourceId::new("db"))
        .with_incident_id(IncidentId::new("inc-7"))
        .with_recovery_run_id(RecoveryRunId::new("run-3"))
        .with_details(SanitizedDetails::new([("attempts", "3")]));
        let json = serde_json::to_string(&event).unwrap();
        let back: NotificationEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn event_omits_absent_optional_ids() {
        let event = NotificationEvent::new(
            EventId::new("evt-2"),
            EventType::ConfigurationInvalid,
            Severity::Warning,
            "bad config",
            1_000,
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("resource_id"), "{json}");
        assert!(!json.contains("incident_id"), "{json}");
        assert!(!json.contains("recovery_run_id"), "{json}");
    }

    #[test]
    fn sanitized_details_are_deterministic_and_flat() {
        let details = SanitizedDetails::new([("b", "2"), ("a", "1")]);
        // BTreeMap keeps keys sorted, so serialization is stable.
        let json = serde_json::to_string(&details).unwrap();
        assert_eq!(json, "{\"a\":\"1\",\"b\":\"2\"}");
        assert_eq!(details.get("a"), Some("1"));
        assert!(SanitizedDetails::default().is_empty());
    }
}

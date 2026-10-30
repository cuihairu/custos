//! The local structured-log channel (docs/guide/notification.md §8.2) —
//! the always-on base audit output. Emits each notification event as one
//! JSON line through `tracing` at a severity-mapped level, so the operator
//! can correlate events without any remote dependency.

use crate::channel::{Channel, ChannelError};
use custos_model::{NotificationEvent, Severity};

/// Local structured-log channel. Writes to `tracing`, never fails.
#[derive(Debug, Clone)]
pub struct LocalLogChannel {
    id: String,
    enabled: bool,
}

impl LocalLogChannel {
    pub fn new(id: impl Into<String>, enabled: bool) -> Self {
        Self {
            id: id.into(),
            enabled,
        }
    }
}

impl Channel for LocalLogChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn deliver(&self, event: &NotificationEvent) -> Result<(), ChannelError> {
        if !self.enabled {
            return Ok(());
        }
        emit(event, self.id());
        Ok(())
    }
}

/// Renders the event as the single structured audit line (JSON). Pure and
/// unit-testable; the channel just hands this to `tracing`.
pub fn format_event(event: &NotificationEvent) -> String {
    serde_json::to_string(event).unwrap_or_else(|_| format!("{:?}", event))
}

/// Maps severity to a `tracing` level so log filters work naturally.
pub fn severity_level(severity: Severity) -> tracing::Level {
    match severity {
        Severity::Info => tracing::Level::INFO,
        Severity::Warning => tracing::Level::WARN,
        Severity::Critical => tracing::Level::ERROR,
    }
}

fn emit(event: &NotificationEvent, channel: &str) {
    let line = format_event(event);
    match severity_level(event.severity) {
        tracing::Level::INFO => {
            tracing::info!(channel, event_id = %event.event_id, event_type = %event.event_type.as_str(), severity = %event.severity.as_str(), "{}", line);
        }
        tracing::Level::WARN => {
            tracing::warn!(channel, event_id = %event.event_id, event_type = %event.event_type.as_str(), severity = %event.severity.as_str(), "{}", line);
        }
        tracing::Level::ERROR => {
            tracing::error!(channel, event_id = %event.event_id, event_type = %event.event_type.as_str(), severity = %event.severity.as_str(), "{}", line);
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_model::{EventId, EventType, SanitizedDetails};

    fn event() -> NotificationEvent {
        NotificationEvent::new(
            EventId::new("evt-1"),
            EventType::RecoveryExhausted,
            Severity::Critical,
            "all attempts exhausted",
            1_700_000_000_000,
        )
        .with_details(SanitizedDetails::new([("attempts", "3")]))
    }

    #[test]
    fn format_event_is_json_with_core_fields() {
        let line = format_event(&event());
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["event_id"], "evt-1");
        assert_eq!(parsed["event_type"], "recovery.exhausted");
        assert_eq!(parsed["severity"], "critical");
        assert_eq!(parsed["summary"], "all attempts exhausted");
        assert_eq!(parsed["details"]["attempts"], "3");
    }

    #[test]
    fn severity_maps_to_levels() {
        assert_eq!(severity_level(Severity::Info), tracing::Level::INFO);
        assert_eq!(severity_level(Severity::Warning), tracing::Level::WARN);
        assert_eq!(severity_level(Severity::Critical), tracing::Level::ERROR);
    }

    #[test]
    fn deliver_is_ok_for_enabled_channel() {
        let channel = LocalLogChannel::new("audit", true);
        assert_eq!(channel.id(), "audit");
        assert!(channel.deliver(&event()).is_ok());
    }

    #[test]
    fn deliver_is_noop_for_disabled_channel() {
        let channel = LocalLogChannel::new("audit", false);
        assert!(channel.deliver(&event()).is_ok());
    }

    /// A process-global capture so the tracing emit is deterministic even
    /// under the parallel test harness (a thread-local default subscriber is
    /// racy across threads). Installed once via `set_global_default`.
    static CAPTURED: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<Vec<String>>>> =
        std::sync::OnceLock::new();
    static INIT: std::sync::Once = std::sync::Once::new();

    #[test]
    fn deliver_emits_structured_line() {
        let captured =
            CAPTURED.get_or_init(|| std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
        INIT.call_once(|| {
            let writer = CapturedWriter(captured.clone());
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || writer.clone())
                .with_max_level(tracing::Level::TRACE)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
        });

        captured.lock().unwrap().clear();
        let channel = LocalLogChannel::new("audit", true);
        channel.deliver(&event()).unwrap();
        let lines = captured.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("recovery.exhausted")),
            "expected a line mentioning the event, got {lines:?}"
        );
    }

    /// Minimal writer capturing formatted lines for assertions.
    #[derive(Clone)]
    struct CapturedWriter(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl std::io::Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let text = String::from_utf8_lossy(buf).to_string();
            self.0.lock().unwrap().push(text);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

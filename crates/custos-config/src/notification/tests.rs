use super::*;
use crate::load;

const BASE: &str = r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"
"#;

#[test]
fn no_notification_section_is_default_empty() {
    let config = load(BASE).unwrap();
    assert!(config.notification.rules.is_empty());
    assert!(config.notification.channels.is_empty());
}

#[test]
fn full_section_parses_with_defaults() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "alert-exhaustion"
event = "recovery.exhausted"
severity_min = "warning"
resource = "web"
channels = ["ops-webhook", "audit"]
log = true

[[notification.rules]]
id = "all-info"
channels = []

[notification.channels.ops-webhook]
type = "webhook"
url = "https://ops.example.com/hook"
headers = { Authorization = "Bearer x" }
hmac_secret = "shh"

[notification.channels.audit]
type = "local-log"

[notification.channels.mail]
type = "smtp"
host = "smtp.example.com"
from = "custos@example.com"
to = ["ops@example.com"]
"#,
    )
    .unwrap();

    let notification = &config.notification;
    assert_eq!(notification.rules.len(), 2);
    let rule = &notification.rules[0];
    assert_eq!(rule.id, "alert-exhaustion");
    assert_eq!(rule.event, Some(EventType::RecoveryExhausted));
    assert_eq!(rule.severity_min, Severity::Warning);
    assert_eq!(rule.resource, Some(ResourceId::new("web")));
    assert_eq!(rule.channels, vec!["ops-webhook", "audit"]);
    assert!(rule.log);

    // Second rule: omitted event/severity/resource mean "everything,
    // info and up, any resource".
    let catch_all = &notification.rules[1];
    assert_eq!(catch_all.event, None);
    assert_eq!(catch_all.severity_min, Severity::Info);
    assert_eq!(catch_all.resource, None);

    // Channel defaults applied.
    match notification.channel("ops-webhook").unwrap() {
        ChannelConfig::Webhook(w) => {
            assert_eq!(w.url, "https://ops.example.com/hook");
            assert_eq!(w.timeout_ms, 10_000);
            assert_eq!(w.retries, 3);
            assert_eq!(w.backoff_ms, 30_000);
            assert!(w.tls_verify);
            assert!(w.ssrf.block_private);
            assert!(w.ssrf.block_loopback);
            assert!(w.ssrf.block_redirects);
        }
        other => panic!("expected webhook, got {other:?}"),
    }
    match notification.channel("audit").unwrap() {
        ChannelConfig::LocalLog { enabled } => assert!(*enabled),
        other => panic!("expected local-log, got {other:?}"),
    }
    match notification.channel("mail").unwrap() {
        ChannelConfig::Smtp(s) => {
            assert_eq!(s.port, 587);
            assert!(s.tls);
        }
        other => panic!("expected smtp, got {other:?}"),
    }
}

#[test]
fn rejects_unknown_event_type() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
event = "resource.exploded"
channels = []
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown event type"), "got: {err}");
}

#[test]
fn rejects_unknown_severity() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
severity_min = "severe"
channels = []
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown severity"), "got: {err}");
}

#[test]
fn rejects_dangling_channel_reference() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
channels = ["ghost"]
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown channel"), "got: {err}");
}

#[test]
fn rejects_unknown_resource_filter() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[[notification.rules]]
id = "r1"
resource = "ghost"
channels = []
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown resource"), "got: {err}");
}

#[test]
fn rejects_unknown_channel_type() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "carrier-pigeon"
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown type"), "got: {err}");
}

#[test]
fn rejects_unknown_field_in_webhook_channel() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "webhook"
url = "https://example.com"
frobnicate = true
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown field"), "got: {err}");
}

#[test]
fn rejects_non_http_webhook_url() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "webhook"
url = "file:///etc/passwd"
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("http or https"), "got: {err}");
}

#[test]
fn rejects_smtp_without_recipients() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "smtp"
host = "smtp.example.com"
from = "a@example.com"
to = []
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("at least one"), "got: {err}");
}

#[test]
fn herald_channel_parses_with_defaults() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.herald]
type = "herald"
base_url = "http://127.0.0.1:8080/"
app = "custos"
token = "trigger-token"
"#,
    )
    .unwrap();
    match config.notification.channel("herald").unwrap() {
        ChannelConfig::Herald(h) => {
            assert_eq!(h.base_url, "http://127.0.0.1:8080");
            assert_eq!(h.app, "custos");
            assert_eq!(h.auth, HeraldAuth::ApiKey);
            assert_eq!(h.category, "alerts");
            assert_eq!(h.audiences, vec!["log"]);
            assert_eq!(h.timeout_ms, 10_000);
            assert_eq!(h.retries, 3);
            assert!(h.token.is_some());
        }
        other => panic!("expected herald, got {other:?}"),
    }
}

#[test]
fn herald_channel_rejects_bad_auth_and_empty_audiences() {
    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.h]
type = "herald"
base_url = "http://127.0.0.1:8080"
app = "custos"
auth = "magic"
"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("api-key or bearer"), "got: {err}");

    let err = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.h]
type = "herald"
base_url = "http://127.0.0.1:8080"
app = "custos"
audiences = []
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("at least one audience"),
        "got: {err}"
    );
}

#[test]
fn secrets_are_redacted_in_debug() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.x]
type = "webhook"
url = "https://example.com"
headers = { Authorization = "Bearer super-secret" }
hmac_secret = "top-secret"
"#,
    )
    .unwrap();
    let debug = format!("{:?}", config.notification.channels.get("x").unwrap());
    assert!(!debug.contains("super-secret"), "leaked headers: {debug}");
    assert!(!debug.contains("top-secret"), "leaked secret: {debug}");
    assert!(debug.contains("[redacted]"), "no redaction: {debug}");
}

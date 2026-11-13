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

#[test]
fn channel_lookup_resolves_configured_ids_and_rejects_unknown() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.audit]
type = "local-log"
enabled = true
"#,
    )
    .unwrap();
    // Dispatch resolves rule channel refs through this accessor; a hit must
    // be the typed channel, a miss must be None (never a default).
    assert_eq!(
        config.notification.channel("audit"),
        Some(&ChannelConfig::LocalLog { enabled: true })
    );
    assert!(config.notification.channel("missing").is_none());
}

#[test]
fn secret_string_redacts_everywhere_but_expose_returns_plaintext() {
    let secret = SecretString::new("hunter2");
    assert_eq!(format!("{secret:?}"), "[redacted]");
    assert_eq!(format!("{secret}"), "[redacted]");
    // The value is reachable only at the exact use site (§8.7).
    assert_eq!(secret.expose(), "hunter2");
}

#[test]
fn smtp_channel_debug_redacts_the_password() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.mail]
type = "smtp"
host = "smtp.example.com"
from = "custos@example.com"
to = ["ops@example.com"]
username = "custos"
password = "mail-hunter2"
"#,
    )
    .unwrap();
    let debug = format!("{:?}", config.notification.channels.get("mail").unwrap());
    assert!(!debug.contains("mail-hunter2"), "leaked password: {debug}");
    // Non-secret fields stay visible for diagnosis.
    assert!(debug.contains("smtp.example.com"), "got: {debug}");
}

#[test]
fn telegram_channel_debug_redacts_the_bot_token() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.tg]
type = "telegram"
bot_token = "tg-hunter2"
chat_id = "@ops"
"#,
    )
    .unwrap();
    let debug = format!("{:?}", config.notification.channels.get("tg").unwrap());
    assert!(!debug.contains("tg-hunter2"), "leaked bot token: {debug}");
    assert!(debug.contains("@ops"), "got: {debug}");
}

#[test]
fn herald_channel_debug_redacts_the_token() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.h]
type = "herald"
base_url = "http://127.0.0.1:8080"
app = "custos"
token = "herald-hunter2"
"#,
    )
    .unwrap();
    let debug = format!("{:?}", config.notification.channels.get("h").unwrap());
    assert!(!debug.contains("herald-hunter2"), "leaked token: {debug}");
    assert!(debug.contains("custos"), "got: {debug}");
}

#[test]
fn local_log_channel_debug_carries_the_enabled_flag() {
    let config = load(
        r#"
version = 1

[[resources]]
id = "web"
type = "http-endpoint"

[notification.channels.off]
type = "local-log"
enabled = false
"#,
    )
    .unwrap();
    match config.notification.channel("off") {
        Some(ChannelConfig::LocalLog { enabled }) => {
            let debug = format!("{:?}", ChannelConfig::LocalLog { enabled: *enabled });
            assert!(debug.contains("enabled: false"), "got: {debug}");
        }
        other => panic!("expected LocalLog channel, got {other:?}"),
    }
}

// The env overlay's pure core: tests drive a map-based variable source and
// never mutate process env (parallel load() tests would observe leftovers).
use std::collections::HashMap;

fn overlay_from_map(map: &[(&str, &str)]) -> Result<NotificationConfig, crate::ConfigError> {
    let owned: HashMap<String, String> = map
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let get = move |key: &str| owned.get(key).cloned();
    let fallback = NotificationConfig::default();
    super::env_overlay_from(get, fallback)
}

fn expect_validation(err: crate::ConfigError) -> String {
    match err {
        crate::ConfigError::Validation { message, .. } => message,
        other => panic!("expected a validation error, got {other}"),
    }
}

#[test]
fn env_overlay_without_selectors_keeps_the_toml_section() {
    let fallback = NotificationConfig::default();
    let out = super::env_overlay_from(|_| None, fallback.clone()).unwrap();
    assert_eq!(out, fallback, "no vars at all: TOML section stands");
    // A lone MIN_SEVERITY is a no-op: only channel selectors trigger the
    // overlay, otherwise setting the knob alone would silently wipe the
    // TOML section.
    let out = overlay_from_map(&[("CUSTOS_NOTIFY_MIN_SEVERITY", "critical")]).unwrap();
    assert_eq!(out, fallback);
}

#[test]
fn env_overlay_builds_a_local_log_rule() {
    let out = overlay_from_map(&[("CUSTOS_NOTIFY_LOCAL_LOG", "1")]).unwrap();
    assert_eq!(out.rules.len(), 1);
    let rule = &out.rules[0];
    assert_eq!(rule.id, "env-notify");
    assert_eq!(rule.severity_min, Severity::Warning, "default floor");
    assert!(rule.log, "local-log channel implies rule.log");
    assert_eq!(rule.channels, ["local-log"]);
    assert_eq!(rule.dedup_window_ms, 30 * 60 * 1000);
    match out.channel("local-log") {
        Some(ChannelConfig::LocalLog { enabled: true }) => {}
        other => panic!("expected enabled LocalLog, got {other:?}"),
    }
}

#[test]
fn env_overlay_local_log_disabled_flag_yields_no_channels() {
    let out = overlay_from_map(&[("CUSTOS_NOTIFY_LOCAL_LOG", "0")]).unwrap();
    assert!(
        out.rules.is_empty() && out.channels.is_empty(),
        "0 disables"
    );
    let out = overlay_from_map(&[("CUSTOS_NOTIFY_LOCAL_LOG", "false")]).unwrap();
    assert!(out.channels.is_empty());
    let out = overlay_from_map(&[("CUSTOS_NOTIFY_LOCAL_LOG", "")]).unwrap();
    assert!(out.channels.is_empty());
}

#[test]
fn env_overlay_wires_telegram_with_defaults() {
    let out = overlay_from_map(&[
        ("CUSTOS_NOTIFY_TELEGRAM_TOKEN", "tok"),
        ("CUSTOS_NOTIFY_TELEGRAM_CHAT", "@custos"),
    ])
    .unwrap();
    let rule = &out.rules[0];
    assert_eq!(rule.channels, ["telegram"]);
    assert!(!rule.log);
    match out.channel("telegram") {
        Some(ChannelConfig::Telegram(cfg)) => {
            assert_eq!(cfg.chat_id, "@custos");
            assert_eq!(cfg.timeout_ms, 10_000);
            assert_eq!(cfg.retries, 3);
            assert_eq!(cfg.backoff_ms, 30_000);
        }
        other => panic!("expected Telegram channel, got {other:?}"),
    }
}

#[test]
fn env_overlay_rejects_a_half_set_telegram_pair() {
    let err = expect_validation(
        overlay_from_map(&[("CUSTOS_NOTIFY_TELEGRAM_TOKEN", "tok")]).unwrap_err(),
    );
    assert!(
        err.contains("TELEGRAM_CHAT"),
        "half-set pair must name the missing var, got: {err}"
    );
    let err =
        expect_validation(overlay_from_map(&[("CUSTOS_NOTIFY_TELEGRAM_CHAT", "@c")]).unwrap_err());
    assert!(err.contains("TELEGRAM_TOKEN"), "got: {err}");
}

#[test]
fn env_overlay_wires_herald_with_defaults() {
    let out = overlay_from_map(&[("CUSTOS_NOTIFY_HERALD_URL", "http://127.0.0.1:8080")]).unwrap();
    match out.channel("herald") {
        Some(ChannelConfig::Herald(cfg)) => {
            assert_eq!(cfg.base_url, "http://127.0.0.1:8080");
            assert_eq!(cfg.app, "custos", "default app id");
            assert_eq!(cfg.category, "alerts");
            assert_eq!(cfg.audiences, ["log"]);
            assert!(cfg.token.is_none());
            assert_eq!(cfg.auth, HeraldAuth::ApiKey);
        }
        other => panic!("expected Herald channel, got {other:?}"),
    }
    // App id and bearer token come through when set.
    let out = overlay_from_map(&[
        ("CUSTOS_NOTIFY_HERALD_URL", "https://herald.example.com"),
        ("CUSTOS_NOTIFY_HERALD_APP", "homegw"),
        ("CUSTOS_NOTIFY_HERALD_TOKEN", "tok"),
    ])
    .unwrap();
    match out.channel("herald") {
        Some(ChannelConfig::Herald(cfg)) => {
            assert_eq!(cfg.app, "homegw");
            assert!(cfg.token.is_some(), "token set");
        }
        other => panic!("expected Herald channel, got {other:?}"),
    }
}

#[test]
fn env_overlay_rejects_a_non_http_herald_url() {
    let err = expect_validation(
        overlay_from_map(&[("CUSTOS_NOTIFY_HERALD_URL", "ftp://127.0.0.1:8080")]).unwrap_err(),
    );
    assert!(err.contains("http or https"), "got: {err}");
}

#[test]
fn env_overlay_rejects_an_unknown_min_severity() {
    let err = expect_validation(
        overlay_from_map(&[
            ("CUSTOS_NOTIFY_LOCAL_LOG", "1"),
            ("CUSTOS_NOTIFY_MIN_SEVERITY", "loudest"),
        ])
        .unwrap_err(),
    );
    assert!(err.contains("unknown severity"), "got: {err}");
    // info and critical parse too.
    let out = overlay_from_map(&[
        ("CUSTOS_NOTIFY_LOCAL_LOG", "1"),
        ("CUSTOS_NOTIFY_MIN_SEVERITY", "critical"),
    ])
    .unwrap();
    assert_eq!(out.rules[0].severity_min, Severity::Critical);
}

#[test]
fn env_overlay_rejects_a_garbage_local_log_flag() {
    let err = expect_validation(
        overlay_from_map(&[("CUSTOS_NOTIFY_LOCAL_LOG", "yes-please")]).unwrap_err(),
    );
    assert!(err.contains("0|1|true|false"), "got: {err}");
}

#[test]
fn env_overlay_replaces_the_whole_toml_section() {
    let fallback = NotificationConfig {
        rules: vec![NotificationRule {
            id: "toml-rule".to_owned(),
            event: None,
            severity_min: Severity::Info,
            resource: None,
            channels: vec![],
            log: false,
            dedup_window_ms: 0,
        }],
        channels: [("off".to_owned(), ChannelConfig::LocalLog { enabled: false })].into(),
    };
    let out = super::env_overlay_from(
        |key| (key == "CUSTOS_NOTIFY_LOCAL_LOG").then(|| "1".to_owned()),
        fallback,
    )
    .map_err(expect_validation)
    .unwrap();
    // The TOML section had one rule and one channel; the overlay must
    // replace both, never merge half-way with the file's leftovers.
    assert_eq!(out.rules.len(), 1);
    assert_eq!(out.rules[0].id, "env-notify");
    assert_eq!(out.channels.len(), 1);
    assert!(out.channel("off").is_none());
}

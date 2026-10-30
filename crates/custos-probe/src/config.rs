//! Shared config readers for the native probes (survey §26.11): each
//! probe reads its `[resources.probe]` table as JSON with conservative
//! defaults, never a shell string.

use serde_json::Value;

/// Reads a required string field.
pub(crate) fn required_str(config: &Value, key: &str) -> Option<String> {
    config[key].as_str().map(str::to_owned)
}

/// Reads an optional string field.
pub(crate) fn optional_str(config: &Value, key: &str) -> Option<String> {
    config[key].as_str().map(str::to_owned)
}

/// Reads a u64 field with a default.
pub(crate) fn u64_or(config: &Value, key: &str, default: u64) -> u64 {
    config[key].as_u64().unwrap_or(default)
}

/// Reads a bool field with a default.
pub(crate) fn bool_or(config: &Value, key: &str, default: bool) -> bool {
    config[key].as_bool().unwrap_or(default)
}

/// Reads a string-array field (e.g. allowed status codes as ranges).
pub(crate) fn string_array(config: &Value, key: &str) -> Vec<String> {
    config[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Reads a service/unit name: non-empty and whitespace-free. A systemd unit
/// or an SCM service name never contains whitespace, which would split argv
/// or break the service-name APIs.
pub(crate) fn service_name(config: &Value, key: &str) -> Option<String> {
    let name = config[key].as_str()?;
    if name.trim().is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }
    Some(name.to_owned())
}

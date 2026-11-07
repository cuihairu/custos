//! Plugin-side server loop (docs/guide/plugins.md §4.3): the other half of
//! the protocol — reads newline JSON-RPC from stdin, dispatches to a
//! handler, writes one reply per line to stdout. stderr is the plugin's
//! diagnostic log and never touches stdout.
//!
//! This keeps plugin binaries thin: they implement semantics (probe,
//! recovery actions), not framing.

use crate::DEFAULT_MAX_MESSAGE_BYTES;
use serde_json::{json, Value};
use std::io::Write;

/// What one dispatched request produced.
pub enum Reply {
    /// A JSON-RPC result.
    Result(Value),
    /// A JSON-RPC error with code and message.
    Error(i64, String),
    /// Reply and exit the loop (shutdown semantics).
    Exit(Value),
}

/// Answers `custos/v1/initialize`.
pub fn initialize_result(plugin_name: &str) -> Reply {
    Reply::Result(json!({
        "protocol_version": crate::rpc::PROTOCOL_VERSION,
        "plugin": plugin_name,
    }))
}

/// Runs the loop: `dispatch` receives (method, params) and answers.
/// Undecodable lines are skipped (§4.4); EOF ends the loop. Each line is
/// capped at [`DEFAULT_MAX_MESSAGE_BYTES`]; an oversized line (a misbehaving
/// host) is drained and skipped so memory stays bounded.
pub fn serve(dispatch: impl Fn(&str, Value) -> Reply) {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut reader = stdin.lock();
    while let Ok(Some(line)) = crate::read_capped_line(&mut reader, DEFAULT_MAX_MESSAGE_BYTES) {
        let trimmed = std::str::from_utf8(&line).ok().map(str::trim);
        let Some(trimmed) = trimmed.filter(|s| !s.is_empty()) else {
            continue;
        };
        let Ok(request) = serde_json::from_str::<Value>(trimmed) else {
            continue; // never crash on garbage (§4.4)
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let reply = dispatch(&method, params);
        let message = match reply {
            Reply::Result(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Reply::Error(code, message) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": code, "message": message}
            }),
            Reply::Exit(result) => {
                let message = json!({"jsonrpc": "2.0", "id": id, "result": result});
                let _ = writeln!(out, "{message}");
                let _ = out.flush();
                return;
            }
        };
        if writeln!(out, "{message}").is_err() {
            return; // host closed the pipe
        }
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_result_carries_the_protocol_version() {
        let Reply::Result(value) = initialize_result("test-plugin 0.1") else {
            panic!("expected result");
        };
        assert_eq!(value["protocol_version"], "1");
        assert_eq!(value["plugin"], "test-plugin 0.1");
    }
}

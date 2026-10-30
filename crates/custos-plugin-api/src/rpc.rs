//! JSON-RPC 2.0 message model for the plugin protocol
//! (docs/guide/plugins.md §4.3).
//!
//! Framing on stdio is newline-delimited JSON: one message per line, hard
//! size cap per line ([`DEFAULT_MAX_MESSAGE_BYTES`]). stdout carries only
//! protocol messages; stderr is plugin diagnostic logging and is NEVER
//! parsed as protocol (§4.4).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version embedded in every handshake.
pub const PROTOCOL_VERSION: &str = "1";

/// Methods are logical names mapped to versioned JSON-RPC method strings
/// (`custos/v1/<name>`), so the wire contract can evolve without breaking
/// old plugins (§4.3).
pub const METHOD_PREFIX: &str = "custos/v1/";

/// The eight logical operations of §4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Initialize,
    DescribeCapabilities,
    Probe,
    Diagnose,
    ListRecoveryActions,
    ExecuteRecoveryAction,
    Verify,
    Shutdown,
}

impl Method {
    /// The versioned wire name, e.g. `custos/v1/initialize`.
    pub fn wire_name(&self) -> String {
        let logical = serde_json::to_string(self)
            .unwrap_or_default()
            .trim_matches('"')
            .to_owned();
        format!("{METHOD_PREFIX}{logical}")
    }

    /// Parses a wire name back into a method; unknown names are an error
    /// the host must answer with `-32601` (§4.4).
    pub fn from_wire_name(name: &str) -> Option<Self> {
        let logical = name.strip_prefix(METHOD_PREFIX)?;
        (logical == "initialize")
            .then_some(Method::Initialize)
            .or((logical == "describe_capabilities").then_some(Method::DescribeCapabilities))
            .or((logical == "probe").then_some(Method::Probe))
            .or((logical == "diagnose").then_some(Method::Diagnose))
            .or((logical == "list_recovery_actions").then_some(Method::ListRecoveryActions))
            .or((logical == "execute_recovery_action").then_some(Method::ExecuteRecoveryAction))
            .or((logical == "verify").then_some(Method::Verify))
            .or((logical == "shutdown").then_some(Method::Shutdown))
    }
}

/// JSON-RPC request id: number or string, never null (a null id cannot be
/// correlated with a response).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

/// A protocol request (§4.3: id, protocol version, method, params,
/// associated resource — and run, when inside a recovery).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: JsonRpc,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// A protocol response: either a result or an error, never both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: JsonRpc,
    pub id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// `"2.0"` — newtype so a wrong version can never be constructed, on the
/// wire or in memory: serialization always emits `2.0` and deserialization
/// rejects anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JsonRpc;

impl Serialize for JsonRpc {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("2.0")
    }
}

impl<'de> Deserialize<'de> for JsonRpc {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "2.0" {
            Ok(JsonRpc)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported jsonrpc version `{value}` (expected 2.0)"
            )))
        }
    }
}

impl JsonRpc {
    pub const fn as_str(self) -> &'static str {
        "2.0"
    }
}

/// JSON-RPC error object. Standard codes plus custos-specific codes in the
/// `-32000` server range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;

    /// The request timed out (host side; also used by well-behaved plugins
    /// for long-running cancellations).
    pub const TIMED_OUT: i32 = -32000;
    /// The request was cancelled before completion.
    pub const CANCELLED: i32 = -32001;
    /// The message exceeded the configured size cap.
    pub const MESSAGE_TOO_LARGE: i32 = -32002;
    /// The plugin is unhealthy or restarting.
    pub const PLUGIN_UNAVAILABLE: i32 = -32003;
    /// Protocol version mismatch during handshake.
    pub const INCOMPATIBLE_PROTOCOL: i32 = -32004;

    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rpc error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

/// Builds a request for `method` with the next sequential id.
pub fn request(id: i64, method: Method, params: Value) -> Request {
    Request {
        jsonrpc: JsonRpc,
        id: RequestId::Number(id),
        method: method.wire_name(),
        params,
    }
}

/// Builds a success response.
pub fn response(id: RequestId, result: Value) -> Response {
    Response {
        jsonrpc: JsonRpc,
        id,
        result: Some(result),
        error: None,
    }
}

/// Builds an error response.
pub fn error_response(id: RequestId, error: RpcError) -> Response {
    Response {
        jsonrpc: JsonRpc,
        id,
        result: None,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_map_to_versioned_wire_names() {
        assert_eq!(Method::Initialize.wire_name(), "custos/v1/initialize");
        assert_eq!(
            Method::ExecuteRecoveryAction.wire_name(),
            "custos/v1/execute_recovery_action"
        );
        for method in [
            Method::Initialize,
            Method::DescribeCapabilities,
            Method::Probe,
            Method::Diagnose,
            Method::ListRecoveryActions,
            Method::ExecuteRecoveryAction,
            Method::Verify,
            Method::Shutdown,
        ] {
            assert_eq!(
                Method::from_wire_name(&method.wire_name()),
                Some(method),
                "roundtrip failed for {method:?}"
            );
        }
    }

    #[test]
    fn unknown_method_names_are_rejected() {
        assert_eq!(Method::from_wire_name("custos/v1/explode"), None);
        assert_eq!(
            Method::from_wire_name("initialize"),
            None,
            "must be versioned"
        );
        assert_eq!(Method::from_wire_name("custos/v2/initialize"), None);
    }

    #[test]
    fn request_serializes_as_jsonrpc_2() {
        let req = request(7, Method::Probe, serde_json::json!({"resource_id": "db"}));
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 7);
        assert_eq!(json["method"], "custos/v1/probe");
        assert_eq!(json["params"]["resource_id"], "db");
    }

    #[test]
    fn response_is_result_or_error_never_both() {
        let ok = response(RequestId::Number(1), serde_json::json!({"healthy": true}));
        let json = serde_json::to_value(&ok).unwrap();
        assert!(json.get("error").is_none() || json["error"].is_null());

        let err = error_response(
            RequestId::String("req-9".into()),
            RpcError::new(RpcError::METHOD_NOT_FOUND, "no such method"),
        );
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["error"]["code"], -32601);
        assert!(json.get("result").is_none() || json["result"].is_null());
        let back: Response = serde_json::from_value(json).unwrap();
        assert_eq!(back.id, RequestId::String("req-9".into()));
    }

    #[test]
    fn a_response_never_carries_result_and_error() {
        // Constructing both is a serialization contract violation; the
        // builder functions make it unrepresentable.
        let r = response(RequestId::Number(1), Value::Null);
        assert!(r.error.is_none());
        let e = error_response(RequestId::Number(1), RpcError::new(0, "x"));
        assert!(e.result.is_none());
    }
}

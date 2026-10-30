//! Plugin protocol models and capability declarations
//! (docs/guide/workspace.md: `custos-plugin-api`).
//!
//! P1 protocol core (docs/guide/plugins.md §4.3–§4.6): versioned JSON-RPC
//! 2.0 method names over newline-delimited stdio, the eight logical
//! operations, capability declarations with the eleven mandatory metadata
//! fields, parameterized operation inputs (never shell text) and wire
//! limits. Process lifecycle lives in `custos-plugin-host`.

pub mod capability;
pub mod limits;
pub mod rpc;
pub mod server;

pub use capability::{AffectedScope, Capability, CapabilityKind, OperationParams};
pub use limits::{DEFAULT_MAX_MESSAGE_BYTES, MAX_STDERR_RETAINED_BYTES};
pub use rpc::{
    error_response, request, response, JsonRpc, Method, Request, RequestId, Response, RpcError,
    METHOD_PREFIX, PROTOCOL_VERSION,
};

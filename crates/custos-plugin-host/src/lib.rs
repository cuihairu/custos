//! Plugin process lifecycle: spawn, handshake, capability negotiation,
//! compatibility validation and graceful shutdown
//! (docs/guide/plugins.md §4.5).
//!
//! The host treats stdout as protocol-only and stderr as plugin diagnostic
//! logging (§4.4): each stream gets an independent, continuously-draining
//! reader so a chatty plugin can never block on a full pipe. The message
//! size cap and error mapping are applied at the protocol boundary.

mod compat;
mod session;

pub use compat::validate_capabilities;
pub use session::{HostError, InitializeResult, PluginConfig, PluginSession};

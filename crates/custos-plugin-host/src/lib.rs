//! Plugin process lifecycle: spawn, handshake, capability negotiation,
//! compatibility validation and graceful shutdown
//! (docs/guide/plugins.md §4.5).
//!
//! The host treats stdout as protocol-only and stderr as plugin diagnostic
//! logging (§4.4): each stream gets an independent, continuously-draining
//! reader so a chatty plugin can never block on a full pipe. The message
//! size cap and error mapping are applied at the protocol boundary.

mod compat;
mod plugin_probe;
mod python;
mod session;
mod supervisor;

pub use compat::{validate_capabilities, CapabilityKindRequirement};
pub use plugin_probe::PluginProbe;
pub use python::{resolve_shim, script_plugin_config, INTERPRETER_ENV, PYTHON_SHIM_PATH, SHIM_ENV};
pub use session::{CallHandle, HostError, InitializeResult, PluginConfig, PluginSession};
pub use supervisor::{PluginStatus, PluginSupervisor, RestartPolicy};

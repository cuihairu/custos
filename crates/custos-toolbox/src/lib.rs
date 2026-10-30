//! The internal integration toolbox (ADR-0015, docs/guide/adr-principles.md
//! §17.15): the core's built-in capability executors, declared with the same
//! `Capability` + `OperationParams` contract plugins use, behind ONE
//! dispatch table.
//!
//! * Built-in tools execute in-process in the core (hardened once: timeouts,
//!   output caps, process-tree cleanup live in the shared primitives).
//! * Plugin-declared operations register in the same table as delegated
//!   entries; dispatch returns [`DispatchedOutcome::ToPlugin`] and the
//!   runtime routes the call to the plugin subprocess.
//! * Dispatch enforces the permission gate (ADR-0012): a capability's
//!   `required_permission` must be granted or the call is refused before
//!   any executor runs.
//!
//! Plugins are the rule layer (targets, parameters, decisions); transports
//! and dangerous primitives are hardened here, once (ADR-0015).

pub mod exec_tool;
pub mod platform_tools;
pub mod process_tool;
pub mod registry;

pub use exec_tool::ExecTool;
pub use platform_tools::{InterfaceResetTool, RebootTool};
pub use process_tool::ProcessStatusTool;
pub use registry::{DispatchedOutcome, ToolOutcome, ToolRegistry, ToolboxError, ToolboxErrorKind};

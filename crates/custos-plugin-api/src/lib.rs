//! Plugin protocol models and capability declarations
//! (docs/guide/workspace.md: `custos-plugin-api`).
//!
//! P1 scope (docs/guide/plugins.md §4.3): versioned JSON-RPC 2.0 over stdio
//! methods — initialize, describe_capabilities, probe, diagnose,
//! list_recovery_actions, execute_recovery_action, verify, shutdown.
//!
//! The user ruling of 2026-10-29 (docs/guide/plugins.md) also requires builtin
//! capabilities for common resources and bidirectional Python access (PyO3
//! outbound, Python scripts inbound under the same plugin contract).

/// Placeholder until the protocol models land in P1.
pub fn not_yet_implemented() -> &'static str {
    "custos-plugin-api: JSON-RPC protocol models are planned for P1 (docs/guide/roadmap.md)"
}

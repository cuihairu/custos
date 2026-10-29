//! Plugin process management, JSON-RPC, timeouts and cancellation
//! (docs/guide/workspace.md: `custos-plugin-host`).
//!
//! P1 scope (docs/guide/plugins.md §4.4): independent continuous stdout/stdio
//! readers, message size limits (1 MiB default), unresponsive-plugin timeout,
//! crash restarts per policy.

/// Placeholder until the host lands in P1.
pub fn not_yet_implemented() -> &'static str {
    "custos-plugin-host: plugin process management is planned for P1 (docs/guide/roadmap.md)"
}

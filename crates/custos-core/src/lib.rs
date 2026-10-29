//! Runtime assembly and resource registration
//! (docs/guide/workspace.md: `custos-core`).
//!
//! Design rule (user ruling, 2026-10-29): keep the core small — the core
//! engine owns the domain model, dependency coordination, budgets and the
//! final verdicts only; everything beyond that is contributed by plugins
//! (builtin adapters stay basic, the ecosystem fills in the rest, see
//! docs/guide/plugins.md).
//!
//! P0 TODO: assemble scheduler, engine, store and notify into the running
//! system; own the resource registry.

/// Placeholder until runtime assembly lands in P0.
pub fn not_yet_implemented() -> &'static str {
    "custos-core: runtime assembly is planned for P0 (docs/guide/roadmap.md)"
}

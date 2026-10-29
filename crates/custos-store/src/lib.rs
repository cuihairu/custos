//! SQLite persistence, migrations, transactions and recovery task state
//! (docs/guide/workspace.md: `custos-store`; schema in
//! docs/guide/persistence.md §7.2).
//!
//! P0 TODO: resources, resource_dependencies, resource_health, incidents,
//! recovery_runs, recovery_steps, probe_results, diagnoses, action_attempts,
//! verification_results, audit_events tables. High-risk actions must persist
//! state before execution and treat post-crash outcomes as unknown (§7.3).

/// Placeholder until SQLite persistence lands in P0.
pub fn not_yet_implemented() -> &'static str {
    "custos-store: SQLite persistence is planned for P0 (docs/guide/roadmap.md)"
}

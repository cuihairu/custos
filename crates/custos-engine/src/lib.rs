//! Recovery engine: dependency scheduling, the recovery state machine,
//! budgets and mutexes (docs/guide/workspace.md: `custos-engine`).
//!
//! The state machine itself already lives in `custos-model::recovery`; this
//! crate owns its execution. P0 TODO (docs/guide/roadmap.md P0 items 8–15):
//! - ordered candidate plan selection driven by diagnosis (§6.3)
//! - re-diagnosis after every action (§6.4)
//! - recovery budgets and termination conditions (§6.5)
//! - per-resource recovery mutexes (§3.7)
//! - unknown-outcome handling after crashes (§6.6)

/// Placeholder until the engine loop lands in P0.
pub fn not_yet_implemented() -> &'static str {
    "custos-engine: recovery orchestration is planned for P0 (docs/guide/roadmap.md)"
}

//! SQLite persistence for recovery state (docs/guide/persistence.md §7).
//!
//! P0 subset of the §7.2 schema: `recovery_runs`, `action_attempts`,
//! `verification_results`, `audit_events`. WAL is enabled on open. The
//! crash-consistency contract (§7.3) lives in the write order the engine
//! already guarantees: attempts are persisted with `outcome = NULL`
//! (`Executing`) before the external action runs, so a post-crash NULL
//! outcome means "result may be unknown", never "action did not run".

mod store;

pub use store::{Store, StoreError};

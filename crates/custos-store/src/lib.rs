//! SQLite persistence for recovery state (docs/guide/persistence.md §7).
//!
//! The §7.2 core schema is applied through versioned migrations
//! ([`migrations`]); WAL is always on and the `synchronous` policy is
//! configurable per §7.1. The crash-consistency contract (§7.3) lives in
//! the write order the engine already guarantees: attempts are persisted
//! with `outcome = NULL` (`Executing`) before the external action runs, so a
//! post-crash NULL outcome means "result may be unknown", never "action did
//! not run". Retention purging (§7.4) never touches active state.

pub mod config;
mod migrations;
mod store;

pub use config::{PurgeReport, RetentionConfig, StoreConfig, SynchronousPolicy};
pub use migrations::MIGRATIONS;
pub use store::{
    AuditRow, Counts, IncidentRow, NotificationCounts, NotificationQueueRow, ProbeRow, RunRow,
    Store, StoreError,
};

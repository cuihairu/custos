//! The SQLite [`RunStore`]: a sync facade over a sqlx pool. The engine stays
//! synchronous; each call blocks on a current-thread tokio runtime owned by
//! the store.

use crate::config::{PurgeReport, RetentionConfig, StoreConfig, SynchronousPolicy};
use crate::migrations::migrate;
use custos_engine::{EngineEvent, RunStore};
use custos_model::ids::RecoveryRunId;
use custos_model::{
    ActionAttempt, NotificationEvent, NotificationState, RecoveryRun, VerificationResult,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{query, query_as, Row, SqlitePool};
use std::future::Future;
use std::path::Path;
use std::sync::Mutex;

mod lifecycle;
mod notify;
mod read;
mod retention;
mod rows;
mod runs;

pub use rows::{
    AuditRow, Counts, IncidentRow, NotificationCounts, NotificationQueueRow, ProbeRow, RunRow,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("corrupt row for `{0}`: {1}")]
    Corrupt(String, #[source] serde_json::Error),
}

type AttemptRow = (String, String, i64, Option<String>, i64, Option<i64>);

/// Parses a persisted `outcome` cell. A present-but-unparsable value is a
/// corrupt row (C2), not "no outcome recorded": the raw text is returned
/// with the error so callers can report it — NULL alone means the attempt
/// was persisted before the outcome existed (crashed mid-run, §7.3).
pub(super) fn parse_outcome(
    cell: Option<&str>,
) -> Result<Option<custos_model::ActionOutcome>, (String, serde_json::Error)> {
    match cell {
        None => Ok(None),
        Some(text) => serde_json::from_str(text)
            .map(Some)
            .map_err(|err| (text.to_owned(), err)),
    }
}

const TERMINAL: &str =
    "('recovered','escalated','suppressed','cancelled','interrupted','outcome_unknown')";

pub struct Store {
    pool: SqlitePool,
    /// Serializes `block_on` calls; a current-thread runtime is not reentrant.
    rt_lock: Mutex<tokio::runtime::Runtime>,
    retention: RetentionConfig,
}
#[cfg(test)]
mod tests;

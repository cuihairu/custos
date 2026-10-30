//! Versioned schema migrations (docs/guide/persistence.md §7.2).
//!
//! Every schema change is a numbered [`Migration`] applied exactly once,
//! recorded in `schema_migrations`. Migrations never edit history: a new
//! change appends a new version. A database created by an older custos
//! (before this framework, schema applied inline with `IF NOT EXISTS`) is
//! treated as version 0 and upgraded in place — the `IF NOT EXISTS` guards
//! make V1 a no-op on such files.

/// One numbered schema change.
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Ordered list of all known schema versions.
pub const MIGRATIONS: &[Migration] = &[V1, V2];

/// V1 — the original P0 schema: recovery state and audit (§7.2 subset).
const V1: Migration = Migration {
    version: 1,
    name: "recovery-state-and-audit",
    sql: "
CREATE TABLE IF NOT EXISTS recovery_runs (
    id                 TEXT PRIMARY KEY,
    resource_id        TEXT NOT NULL,
    incident_id        TEXT,
    status             TEXT NOT NULL,
    trigger            TEXT NOT NULL,
    started_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    payload            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_recovery_runs_status ON recovery_runs(status);

CREATE TABLE IF NOT EXISTS action_attempts (
    run_id              TEXT NOT NULL REFERENCES recovery_runs(id),
    action_id           TEXT NOT NULL,
    plan_id             TEXT NOT NULL,
    attempt_no          INTEGER NOT NULL,
    outcome             TEXT,
    started_at_unix_ms  INTEGER NOT NULL,
    finished_at_unix_ms INTEGER,
    PRIMARY KEY (run_id, action_id)
);

CREATE TABLE IF NOT EXISTS verification_results (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    resource_id        TEXT NOT NULL,
    passed             INTEGER NOT NULL,
    summary            TEXT NOT NULL,
    evidence_refs      TEXT NOT NULL,
    checked_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at_unix_ms  INTEGER NOT NULL,
    event_type  TEXT NOT NULL,
    severity    TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    run_id      TEXT,
    payload     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_events_at ON audit_events(at_unix_ms);
",
};

/// V2 — the remaining §7.2 core tables: registry, evidence, incidents,
/// notifications, plugins and configuration history.
const V2: Migration = Migration {
    version: 2,
    name: "full-core-schema",
    sql: "
CREATE TABLE IF NOT EXISTS resources (
    id                 TEXT PRIMARY KEY,
    resource_type      TEXT NOT NULL,
    display_name       TEXT NOT NULL,
    enabled            INTEGER NOT NULL,
    config_json        TEXT NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS resource_dependencies (
    resource_id         TEXT NOT NULL,
    target_resource_id  TEXT NOT NULL,
    requirement         TEXT NOT NULL,
    condition_json      TEXT,
    PRIMARY KEY (resource_id, target_resource_id)
);

CREATE TABLE IF NOT EXISTS resource_health (
    resource_id             TEXT PRIMARY KEY,
    status                  TEXT NOT NULL,
    last_changed_unix_ms    INTEGER NOT NULL,
    consecutive_failures    INTEGER NOT NULL,
    consecutive_successes   INTEGER NOT NULL,
    snapshot_json           TEXT NOT NULL,
    updated_at_unix_ms      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS incidents (
    id                TEXT PRIMARY KEY,
    resource_id       TEXT NOT NULL,
    fault_code        TEXT NOT NULL,
    summary           TEXT NOT NULL,
    opened_at_unix_ms INTEGER NOT NULL,
    closed_at_unix_ms INTEGER
);
CREATE INDEX IF NOT EXISTS idx_incidents_resource ON incidents(resource_id);

CREATE TABLE IF NOT EXISTS recovery_steps (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id       TEXT NOT NULL REFERENCES recovery_runs(id),
    step_no      INTEGER NOT NULL,
    phase        TEXT NOT NULL,
    detail       TEXT NOT NULL,
    at_unix_ms   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_recovery_steps_run ON recovery_steps(run_id);

CREATE TABLE IF NOT EXISTS probe_results (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    resource_id        TEXT NOT NULL,
    probe_name         TEXT NOT NULL,
    succeeded          INTEGER NOT NULL,
    observation        TEXT NOT NULL,
    collected_at_unix_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_probe_results_at ON probe_results(collected_at_unix_ms);

CREATE TABLE IF NOT EXISTS diagnoses (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id        TEXT,
    resource_id   TEXT NOT NULL,
    fault_code    TEXT NOT NULL,
    summary       TEXT NOT NULL,
    evidence_refs TEXT NOT NULL,
    at_unix_ms    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_diagnoses_resource ON diagnoses(resource_id);

CREATE TABLE IF NOT EXISTS notification_queue (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    resource_id        TEXT,
    channel            TEXT NOT NULL,
    payload            TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    delivered_at_unix_ms INTEGER
);

CREATE TABLE IF NOT EXISTS notification_attempts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    notification_id INTEGER NOT NULL REFERENCES notification_queue(id),
    outcome         TEXT NOT NULL,
    detail          TEXT,
    at_unix_ms      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_registry (
    name               TEXT PRIMARY KEY,
    version            TEXT NOT NULL,
    capabilities       TEXT NOT NULL,
    loaded_at_unix_ms  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS configuration_versions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    version          INTEGER NOT NULL,
    digest           TEXT NOT NULL,
    summary          TEXT NOT NULL,
    applied_at_unix_ms INTEGER NOT NULL
);
",
};

/// The bookkeeping table itself. Kept out of the numbered migrations so a
/// pre-framework database (no table, version 0) upgrades cleanly.
const ENSURE_MIGRATIONS_TABLE: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations (
    version            INTEGER PRIMARY KEY,
    name               TEXT NOT NULL,
    applied_at_unix_ms INTEGER NOT NULL
);
";

/// Applies pending migrations in order, each in its own transaction, and
/// records it. Returns the versions applied (empty on an up-to-date DB).
/// A database without `schema_migrations` is version 0: every migration
/// runs, and the `IF NOT EXISTS` guards make re-applying V1 a no-op.
pub(crate) async fn migrate(pool: sqlx::SqlitePool) -> Result<Vec<u32>, sqlx::Error> {
    use sqlx::query;
    use sqlx::Row;
    query(ENSURE_MIGRATIONS_TABLE).execute(&pool).await?;
    let applied: Vec<i64> = query("SELECT version FROM schema_migrations")
        .fetch_all(&pool)
        .await?
        .into_iter()
        .map(|row: sqlx::sqlite::SqliteRow| row.get::<i64, _>(0))
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut newly = Vec::new();
    for migration in MIGRATIONS {
        if applied.contains(&(migration.version as i64)) {
            continue;
        }
        let mut tx = pool.begin().await?;
        query(migration.sql).execute(&mut *tx).await?;
        query("INSERT INTO schema_migrations (version, name, applied_at_unix_ms) VALUES (?, ?, ?)")
            .bind(migration.version)
            .bind(migration.name)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        newly.push(migration.version);
    }
    Ok(newly)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_strictly_increasing() {
        for pair in MIGRATIONS.windows(2) {
            assert!(
                pair[0].version < pair[1].version,
                "migration versions must increase: {} then {}",
                pair[0].version,
                pair[1].version
            );
        }
    }

    #[test]
    fn migrations_have_names() {
        for m in MIGRATIONS {
            assert!(!m.name.is_empty(), "v{} needs a name", m.version);
            assert!(!m.sql.is_empty(), "v{} needs SQL", m.version);
        }
    }
}

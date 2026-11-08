//! Store concern: connection lifecycle, migration application and blocking helpers.

use super::*;

impl Store {
    /// Opens (creating if needed) with the default config: WAL, `synchronous
    /// = FULL`, nothing ever purged.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with(path, &StoreConfig::default())
    }
    /// Opens (creating if needed) the database at `path` with WAL enabled,
    /// the configured `synchronous` policy (§7.1), and all pending schema
    /// migrations applied (§7.2).
    pub fn open_with(path: impl AsRef<Path>, config: &StoreConfig) -> Result<Self, StoreError> {
        let synchronous = match config.synchronous {
            SynchronousPolicy::Off => SqliteSynchronous::Off,
            SynchronousPolicy::Normal => SqliteSynchronous::Normal,
            SynchronousPolicy::Full => SqliteSynchronous::Full,
        };
        let options = SqliteConnectOptions::new()
            .filename(path.as_ref())
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(synchronous)
            .create_if_missing(true);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| StoreError::Sqlx(sqlx::Error::Io(std::io::Error::other(e))))?;
        let pool = rt.block_on(
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options),
        )?;
        let store = Self {
            pool,
            rt_lock: Mutex::new(rt),
            retention: config.retention,
        };
        store.block(|pool| async move { migrate(pool).await.map(|_| ()) })?;
        Ok(store)
    }
    /// Schema versions currently applied, for `custos status` and tests.
    pub fn applied_migrations(&self) -> Result<Vec<(i64, String, i64)>, StoreError> {
        Ok(self.block(|pool| async move {
            query_as::<_, (i64, String, i64)>(
                "SELECT version, name, applied_at_unix_ms \
                     FROM schema_migrations ORDER BY version",
            )
            .fetch_all(&pool)
            .await
        })?)
    }
    /// Runs `f` on the store's runtime. Futures need not be `Send`:
    /// the runtime is single-threaded and the pool has one connection.
    pub(super) fn block<T, F, Fut>(&self, f: F) -> Result<T, sqlx::Error>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = Result<T, sqlx::Error>>,
    {
        let rt = self.rt_lock.lock().unwrap_or_else(|e| e.into_inner());
        rt.block_on(f(self.pool.clone()))
    }
}

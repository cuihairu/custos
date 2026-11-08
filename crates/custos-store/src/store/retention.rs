//! Store concern: retention purging of expired inactive rows.

use super::*;

impl Store {
    /// Applies the retention policy (§7.4) as of `now_unix_ms`. Active state
    /// is never touched: open incidents, non-terminal runs and undelivered
    /// notifications are excluded by the WHERE clauses, whatever their age.
    pub fn purge_retention(&self, now_unix_ms: u64) -> Result<PurgeReport, StoreError> {
        let retention = self.retention;
        self.block(|pool| async move {
            let mut report = PurgeReport::default();

            if let Some(cutoff) = retention.audit_cutoff(now_unix_ms) {
                report.audit_events = query("DELETE FROM audit_events WHERE at_unix_ms < ?")
                    .bind(cutoff)
                    .execute(&pool)
                    .await?
                    .rows_affected();
            }

            if let Some(cutoff) = retention.probe_cutoff(now_unix_ms) {
                report.probe_results = query("DELETE FROM probe_results WHERE collected_at_unix_ms < ?")
                    .bind(cutoff)
                    .execute(&pool)
                    .await?
                    .rows_affected();
            }

            if let Some(run_cutoff) = retention.run_cutoff(now_unix_ms) {
                // Attempts first: they reference the runs being removed.
                report.attempts = query(&format!(
                    "DELETE FROM action_attempts WHERE run_id IN (\
                     SELECT id FROM recovery_runs \
                     WHERE updated_at_unix_ms < ? AND status IN {TERMINAL})"
                ))
                .bind(run_cutoff)
                .execute(&pool)
                .await?
                .rows_affected();
                report.verifications =
                    query("DELETE FROM verification_results WHERE checked_at_unix_ms < ?")
                        .bind(run_cutoff)
                        .execute(&pool)
                        .await?
                        .rows_affected();
                report.runs = query(&format!(
                    "DELETE FROM recovery_runs WHERE updated_at_unix_ms < ? AND status IN {TERMINAL}"
                ))
                .bind(run_cutoff)
                .execute(&pool)
                .await?
                .rows_affected();
                // Closed incidents only; open ones stay until resolved.
                report.incidents =
                    query("DELETE FROM incidents WHERE closed_at_unix_ms IS NOT NULL AND closed_at_unix_ms < ?")
                        .bind(run_cutoff)
                        .execute(&pool)
                        .await?
                        .rows_affected();
            }

            if let Some(cutoff) = retention.notification_cutoff(now_unix_ms) {
                report.notification_attempts =
                    query("DELETE FROM notification_attempts WHERE at_unix_ms < ?")
                        .bind(cutoff)
                        .execute(&pool)
                        .await?
                        .rows_affected();
                // Delivered only; undelivered notifications must survive
                // offline periods of any length (§7.4).
                report.delivered_notifications = query(
                    "DELETE FROM notification_queue \
                     WHERE delivered_at_unix_ms IS NOT NULL AND delivered_at_unix_ms < ?",
                )
                .bind(cutoff)
                .execute(&pool)
                .await?
                .rows_affected();
            }

            Ok(report)
        })
        .map_err(Into::into)
    }
}

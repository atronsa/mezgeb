//! Concurrency control (step 1.10, ADR-0007).
//!
//! Default scheme: `READ COMMITTED`, and every account an operation touches
//! is locked with `SELECT … FOR NO KEY UPDATE` in ascending ID order before
//! its balance is checked. A deadlock (`40P01`) or serialization failure
//! (`40001`) re-runs the whole transaction, at most `max_attempts` times in
//! total, with jittered backoff. Waiting for a lock is capped by
//! `lock_timeout`.
//!
//! The isolation level and the row locks are options only so that the
//! alternatives in ADR-0007 can be measured on the same workload.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mezgeb_core::LedgerError;
use sqlx::{Acquire, PgConnection, Postgres, Transaction};

use crate::rows::db;

/// The transaction isolation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    ReadCommitted,
    Serializable,
}

/// How postings run under concurrency.
#[derive(Debug, Clone)]
pub struct PostOptions {
    pub isolation: Isolation,
    /// Lock the entry's accounts with `FOR NO KEY UPDATE` before checking
    /// balances. Turning this off is only for measuring the alternatives:
    /// without it, `READ COMMITTED` checks balances that may already be
    /// stale.
    pub lock_rows: bool,
    /// Longest wait for a row lock before giving up with `Busy`.
    pub lock_timeout: Duration,
    /// Attempts in total, including the first, on deadlock or serialization
    /// failure.
    pub max_attempts: u32,
    /// Counts attempts and retries, if set.
    pub stats: Option<Arc<RetryStats>>,
    /// Test hook (feature `test-hooks`): after a posting commits, report a
    /// lost connection instead of the result, as if the response never
    /// reached the client.
    #[cfg(feature = "test-hooks")]
    pub lose_response_after_commit: bool,
}

impl Default for PostOptions {
    fn default() -> PostOptions {
        PostOptions {
            isolation: Isolation::ReadCommitted,
            lock_rows: true,
            lock_timeout: Duration::from_secs(5),
            max_attempts: 3,
            stats: None,
            #[cfg(feature = "test-hooks")]
            lose_response_after_commit: false,
        }
    }
}

/// Counters for measuring contention.
#[derive(Debug, Default)]
pub struct RetryStats {
    /// Transactions started, including retries.
    pub attempts: AtomicU64,
    /// Retries after a deadlock (`40P01`).
    pub deadlock_retries: AtomicU64,
    /// Retries after a serialization failure (`40001`).
    pub serialization_retries: AtomicU64,
    /// Operations that still conflicted after the last attempt.
    pub gave_up: AtomicU64,
}

impl RetryStats {
    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }

    pub fn deadlock_retries(&self) -> u64 {
        self.deadlock_retries.load(Ordering::Relaxed)
    }

    pub fn serialization_retries(&self) -> u64 {
        self.serialization_retries.load(Ordering::Relaxed)
    }

    pub fn gave_up(&self) -> u64 {
        self.gave_up.load(Ordering::Relaxed)
    }
}

impl PostOptions {
    /// Runs after a posting commits. Without the `test-hooks` feature it
    /// does nothing.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn after_commit(&self) -> Result<(), LedgerError> {
        if self.lose_response_after_commit {
            return Err(LedgerError::Storage(
                "test hook: connection lost after commit, response not delivered".into(),
            ));
        }
        Ok(())
    }

    /// Runs after a posting commits. Without the `test-hooks` feature it
    /// does nothing.
    #[cfg(not(feature = "test-hooks"))]
    pub(crate) fn after_commit(&self) -> Result<(), LedgerError> {
        Ok(())
    }

    /// The lock clause for loading accounts.
    pub(crate) fn lock_clause(&self) -> &'static str {
        if self.lock_rows {
            " FOR NO KEY UPDATE"
        } else {
            ""
        }
    }

    /// Starts a transaction with this isolation level and lock timeout.
    pub(crate) async fn begin<'c>(
        &self,
        conn: &'c mut PgConnection,
    ) -> Result<Transaction<'c, Postgres>, LedgerError> {
        if let Some(stats) = &self.stats {
            stats.attempts.fetch_add(1, Ordering::Relaxed);
        }
        let mut tx = conn.begin().await.map_err(db)?;
        if self.isolation == Isolation::Serializable {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        sqlx::query("SELECT set_config('lock_timeout', $1, true)")
            .bind(format!("{}ms", self.lock_timeout.as_millis()))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        Ok(tx)
    }

    /// Decides whether to retry after `error` on attempt number `attempt`
    /// (starting at 1), and waits before the retry if so.
    pub(crate) async fn retry_after(&self, error: &LedgerError, attempt: u32) -> bool {
        let LedgerError::Contention(code) = error else {
            return false;
        };
        if attempt >= self.max_attempts {
            if let Some(stats) = &self.stats {
                stats.gave_up.fetch_add(1, Ordering::Relaxed);
            }
            return false;
        }
        if let Some(stats) = &self.stats {
            let counter = if code == "40P01" {
                &stats.deadlock_retries
            } else {
                &stats.serialization_retries
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
        tokio::time::sleep(backoff(attempt)).await;
        true
    }
}

/// 5 ms, then 10 ms, 20 ms …, plus up to the same again at random, so that
/// transactions that collided do not collide again in lockstep.
fn backoff(attempt: u32) -> Duration {
    let base = 5u64 << attempt.saturating_sub(1).min(6);
    let noise = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    Duration::from_millis(base + noise % (base + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_bounded() {
        for attempt in 1..10 {
            let base = 5u64 << (attempt - 1).min(6);
            let wait = backoff(attempt).as_millis() as u64;
            assert!(
                (base..=2 * base).contains(&wait),
                "attempt {attempt}: {wait} ms"
            );
        }
    }

    #[test]
    fn default_options_follow_adr_0007() {
        let options = PostOptions::default();
        assert_eq!(options.isolation, Isolation::ReadCommitted);
        assert!(options.lock_rows);
        assert_eq!(options.lock_clause(), " FOR NO KEY UPDATE");
        assert_eq!(options.max_attempts, 3);
        let unlocked = PostOptions {
            lock_rows: false,
            ..PostOptions::default()
        };
        assert_eq!(unlocked.lock_clause(), "");
    }
}

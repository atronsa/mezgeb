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
    /// Test hook (feature `test-hooks`): make every write with these
    /// options fail at this point (step 1.15).
    #[cfg(feature = "test-hooks")]
    pub fault: Option<FaultPoint>,
}

/// A point in a write where a test can make it fail (step 1.15). Only takes
/// effect with the `test-hooks` feature, through `PostOptions::fault`.
///
/// Every write (a posting, a reversal, an adjustment, a hold operation, an
/// expiry by the worker) passes all four points in order. A failure before
/// the commit is a crash: the transaction rolls back and nothing is written.
/// A failure after it is a lost response: everything is written, but the
/// caller sees an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint {
    /// The transaction has started and the idempotency key is claimed, but
    /// no account is locked yet.
    BeforeLock,
    /// The accounts are locked and updated, and the entry and its lines are
    /// inserted.
    AfterLines,
    /// Everything is written; `COMMIT` is next.
    BeforeCommit,
    /// `COMMIT` succeeded, and the result is about to be returned.
    AfterCommit,
}

impl FaultPoint {
    /// All four, in the order a write passes them.
    pub const ALL: [FaultPoint; 4] = [
        FaultPoint::BeforeLock,
        FaultPoint::AfterLines,
        FaultPoint::BeforeCommit,
        FaultPoint::AfterCommit,
    ];
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
            fault: None,
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
    /// Fails here if a test asked for it. Without the `test-hooks` feature
    /// it does nothing.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn fault_at(&self, point: FaultPoint) -> Result<(), LedgerError> {
        if self.fault != Some(point) {
            return Ok(());
        }
        Err(LedgerError::Storage(match point {
            FaultPoint::AfterCommit => {
                "test hook: connection lost after commit, response not delivered".into()
            }
            _ => format!("test hook: crashed at {point:?}"),
        }))
    }

    /// Fails here if a test asked for it. Without the `test-hooks` feature
    /// it does nothing.
    #[cfg(not(feature = "test-hooks"))]
    #[allow(clippy::unused_self)]
    pub(crate) fn fault_at(&self, _point: FaultPoint) -> Result<(), LedgerError> {
        Ok(())
    }

    /// Commits a write's transaction, passing the last two fault points.
    pub(crate) async fn commit(&self, tx: Transaction<'_, Postgres>) -> Result<(), LedgerError> {
        self.fault_at(FaultPoint::BeforeCommit)?;
        tx.commit().await.map_err(db)?;
        self.fault_at(FaultPoint::AfterCommit)
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

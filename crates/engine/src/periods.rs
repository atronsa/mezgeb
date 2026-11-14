//! Accounting periods (step 2.05, INV-10, migration 0007).
//!
//! Every entry belongs to the period that was open when it was posted. A
//! period is **open** while postings go to it, **closing** once the next
//! period has taken over and it is being verified, and **closed** once its
//! snapshots are written: from then on its balances never change.
//!
//! Exactly one period is open at a time. Each posting takes a shared
//! advisory lock before it reads the open period, and holds it until it
//! commits. Closing a period takes the same lock exclusively for an instant
//! to switch to the next one, so it waits for every posting already under
//! way, and every entry of a period is committed before the period stops
//! taking postings. A period therefore holds a contiguous range of entry
//! sequences: `after_sequence < sequence <= last_sequence`.
//!
//! A late posting (for example, an offline payment from last week) goes to
//! the current open period. Its original date is its `event_time`.

use std::fmt;

use mezgeb_core::{LedgerError, Timestamp};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};

use crate::rows::{db, storage};

/// The advisory lock key that postings take shared and the period switch
/// takes exclusively ("mezgeb_p" in ASCII).
pub(crate) const PERIOD_LOCK: i64 = 0x6d65_7a67_6562_5f70;

/// Where a period is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodStatus {
    /// Postings go to it.
    Open,
    /// The next period has taken over; it is being verified.
    Closing,
    /// Verified and snapshotted. Its balances never change.
    Closed,
}

impl PeriodStatus {
    /// The name used in storage.
    pub const fn as_str(self) -> &'static str {
        match self {
            PeriodStatus::Open => "open",
            PeriodStatus::Closing => "closing",
            PeriodStatus::Closed => "closed",
        }
    }

    /// The inverse of [`PeriodStatus::as_str`].
    pub fn from_name(name: &str) -> Option<PeriodStatus> {
        [
            PeriodStatus::Open,
            PeriodStatus::Closing,
            PeriodStatus::Closed,
        ]
        .into_iter()
        .find(|s| s.as_str() == name)
    }
}

impl fmt::Display for PeriodStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One accounting period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Period {
    pub id: i64,
    /// The operator's name for it, such as `2026-11-14`.
    pub label: String,
    pub status: PeriodStatus,
    pub opened_at: Timestamp,
    /// The period holds the entries with `after_sequence < sequence`...
    pub after_sequence: u64,
    /// ...and `sequence <= last_sequence`, once it has stopped taking
    /// postings.
    pub last_sequence: Option<u64>,
    pub closing_at: Option<Timestamp>,
    pub closed_at: Option<Timestamp>,
}

pub(crate) const PERIOD_COLUMNS: &str = "id, label, status, \
     (extract(epoch FROM opened_at) * 1000000)::bigint AS opened_micros, \
     after_sequence, last_sequence, \
     (extract(epoch FROM closing_at) * 1000000)::bigint AS closing_micros, \
     (extract(epoch FROM closed_at) * 1000000)::bigint AS closed_micros";

fn sequence(value: i64) -> Result<u64, LedgerError> {
    u64::try_from(value).map_err(storage)
}

pub(crate) fn period_from_row(row: &PgRow) -> Result<Period, LedgerError> {
    let status: String = row.try_get("status").map_err(db)?;
    let last: Option<i64> = row.try_get("last_sequence").map_err(db)?;
    let closing: Option<i64> = row.try_get("closing_micros").map_err(db)?;
    let closed: Option<i64> = row.try_get("closed_micros").map_err(db)?;
    Ok(Period {
        id: row.try_get("id").map_err(db)?,
        label: row.try_get("label").map_err(db)?,
        status: PeriodStatus::from_name(&status)
            .ok_or_else(|| storage("stored period status is invalid"))?,
        opened_at: Timestamp::from_micros(row.try_get("opened_micros").map_err(db)?),
        after_sequence: sequence(row.try_get("after_sequence").map_err(db)?)?,
        last_sequence: last.map(sequence).transpose()?,
        closing_at: closing.map(Timestamp::from_micros),
        closed_at: closed.map(Timestamp::from_micros),
    })
}

/// The period a posting goes to: takes the shared period lock, held until
/// the posting's transaction ends, then reads the open period.
pub(crate) async fn posting_period(conn: &mut PgConnection) -> Result<i64, LedgerError> {
    // Two statements: the lock must be held before the period is read.
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
        .bind(PERIOD_LOCK)
        .execute(&mut *conn)
        .await
        .map_err(db)?;
    sqlx::query_scalar("SELECT id FROM periods WHERE status = 'open'")
        .fetch_optional(&mut *conn)
        .await
        .map_err(db)?
        .ok_or(LedgerError::NoOpenPeriod)
}

/// The open period, if any.
pub async fn current_period(conn: &mut PgConnection) -> Result<Option<Period>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {PERIOD_COLUMNS} FROM periods WHERE status = 'open'"
    )))
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(period_from_row).transpose()
}

/// Every period, oldest first.
pub async fn all_periods(conn: &mut PgConnection) -> Result<Vec<Period>, LedgerError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {PERIOD_COLUMNS} FROM periods ORDER BY id"
    )))
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter().map(period_from_row).collect()
}

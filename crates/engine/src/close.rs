//! Closing an accounting period (step 2.05, INV-10). The operator's steps
//! are in `docs/runbooks/close-period.md`.
//!
//! [`close_period`] closes the open period in three steps, each its own
//! transaction:
//!
//! 1. **Switch.** Take the period lock exclusively, which waits for every
//!    posting under way to commit. Mark the open period `closing`, record
//!    its last entry sequence, and open the next period. From here on,
//!    every new posting goes to the next period.
//! 2. **Verify.** Check every entry of the period: it lies in the period's
//!    range, is balanced in every currency, and nothing posted after the
//!    switch claims the period. Compute each account's totals at the end of
//!    the period two independent ways, from the previous snapshot plus the
//!    period's lines and from the stored running totals on each account's
//!    last line, and check they agree.
//! 3. **Snapshot and close.** Write the totals as the period's snapshots and
//!    mark it `closed`. Its balances never change again.
//!
//! If verification finds a problem, the period stays `closing`, nothing is
//! written, and the findings are returned: postings continue in the next
//! period while the problem is investigated. Calling [`close_period`] again
//! retries the same period. A close that stopped part-way, for whatever
//! reason, is finished the same way.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mezgeb_core::{AccountId, LedgerError};
use sqlx::{Acquire, PgConnection, Row};
use uuid::Uuid;

use crate::periods::{PERIOD_COLUMNS, PERIOD_LOCK, Period, period_from_row};
use crate::rows::{account_id, db, storage};

/// A problem that stops a period from closing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeriodFinding {
    /// An entry of the period lies outside its sequence range, or an entry
    /// in its range belongs to another period.
    EntryOutsideRange {
        entry: Uuid,
        sequence: i64,
        period: Option<i64>,
    },
    /// An entry of the period does not balance in a currency (INV-1).
    EntryUnbalanced {
        entry: Uuid,
        currency: String,
        debits: i64,
        credits: i64,
    },
    /// An account's totals from the previous snapshot plus the period's
    /// lines differ from the running totals on its last line (INV-5, INV-6).
    TotalsDisagree {
        account: Uuid,
        from_lines: (i64, i64),
        stored: (i64, i64),
    },
}

impl fmt::Display for PeriodFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeriodFinding::EntryOutsideRange {
                entry,
                sequence,
                period,
            } => write!(
                f,
                "entry {entry} (sequence {sequence}, period {period:?}) is outside the period's range"
            ),
            PeriodFinding::EntryUnbalanced {
                entry,
                currency,
                debits,
                credits,
            } => write!(
                f,
                "INV-1 entry {entry}: {currency} debits {debits} ≠ credits {credits}"
            ),
            PeriodFinding::TotalsDisagree {
                account,
                from_lines,
                stored,
            } => write!(
                f,
                "INV-5 account {account}: totals from lines {from_lines:?} ≠ stored running totals {stored:?}"
            ),
        }
    }
}

/// One account's totals at the end of a closed period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub account: AccountId,
    pub debits_total: i64,
    pub credits_total: i64,
}

/// What a close did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseReport {
    /// The period that was closed, or that is still closing if there are
    /// findings.
    pub period: Period,
    /// The period postings now go to.
    pub next: Option<Period>,
    /// Entries checked in the period.
    pub entries: i64,
    /// Accounts snapshotted.
    pub snapshots: usize,
    /// Empty if the period is closed.
    pub findings: Vec<PeriodFinding>,
}

impl CloseReport {
    pub fn is_closed(&self) -> bool {
        self.findings.is_empty()
    }
}

async fn period_where(
    conn: &mut PgConnection,
    condition: &str,
) -> Result<Option<Period>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {PERIOD_COLUMNS} FROM periods WHERE {condition}"
    )))
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(period_from_row).transpose()
}

/// Step 1: stops postings to the open period and opens `next_label`.
/// Returns the period that is now closing. Does nothing if a period is
/// already closing (a close that stopped part-way), and returns it.
pub async fn begin_close(conn: &mut PgConnection, next_label: &str) -> Result<Period, LedgerError> {
    let mut tx = conn.begin().await.map_err(db)?;
    // Waits for every posting under way, which holds the lock shared.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(PERIOD_LOCK)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    if let Some(closing) = period_where(&mut tx, "status = 'closing'").await? {
        return Ok(closing);
    }
    let open = period_where(&mut tx, "status = 'open' FOR UPDATE")
        .await?
        .ok_or(LedgerError::NoOpenPeriod)?;
    // Every posting of the period has committed, and none can start: the
    // period ends at the highest sequence there is.
    let last: i64 =
        sqlx::query_scalar("SELECT greatest(coalesce(max(sequence), 0), $1) FROM journal_entries")
            .bind(i64::try_from(open.after_sequence).map_err(storage)?)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
    sqlx::query(
        "UPDATE periods SET status = 'closing', closing_at = clock_timestamp(), last_sequence = $2
          WHERE id = $1",
    )
    .bind(open.id)
    .bind(last)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    sqlx::query("INSERT INTO periods (label, status, after_sequence) VALUES ($1, 'open', $2)")
        .bind(next_label)
        .bind(last)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let closing = period_where(&mut tx, &format!("id = {}", open.id))
        .await?
        .ok_or_else(|| storage("the closing period disappeared"))?;
    tx.commit().await.map_err(db)?;
    Ok(closing)
}

/// Steps 2 and 3: verifies the closing period and, if it is clean, writes
/// its snapshots and closes it. With findings, nothing is written.
pub async fn finish_close(conn: &mut PgConnection) -> Result<CloseReport, LedgerError> {
    let mut tx = conn.begin().await.map_err(db)?;
    // One consistent view of the ledger for every check.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let period = period_where(&mut tx, "status = 'closing' FOR UPDATE")
        .await?
        .ok_or_else(|| storage("no period is closing"))?;
    let after = i64::try_from(period.after_sequence).map_err(storage)?;
    let last = period
        .last_sequence
        .map(i64::try_from)
        .transpose()
        .map_err(storage)?
        .ok_or_else(|| storage("a closing period has no last sequence"))?;
    let mut findings = Vec::new();

    // The period's entries, and only those, lie in its range.
    let outside = sqlx::query(
        "SELECT id, sequence, period_id FROM journal_entries
          WHERE (period_id = $1 AND NOT (sequence > $2 AND sequence <= $3))
             OR (sequence > $2 AND sequence <= $3 AND period_id IS DISTINCT FROM $1)
          ORDER BY sequence",
    )
    .bind(period.id)
    .bind(after)
    .bind(last)
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;
    for row in &outside {
        findings.push(PeriodFinding::EntryOutsideRange {
            entry: row.try_get("id").map_err(db)?,
            sequence: row.try_get("sequence").map_err(db)?,
            period: row.try_get("period_id").map_err(db)?,
        });
    }

    // Each entry of the period balances in every currency (INV-1).
    let unbalanced = sqlx::query(
        "SELECT l.entry_id, l.currency,
                coalesce(sum(l.amount) FILTER (WHERE l.side = 'debit'), 0)::bigint AS debits,
                coalesce(sum(l.amount) FILTER (WHERE l.side = 'credit'), 0)::bigint AS credits
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE e.sequence > $1 AND e.sequence <= $2
          GROUP BY l.entry_id, l.currency
         HAVING coalesce(sum(l.amount) FILTER (WHERE l.side = 'debit'), 0)
             <> coalesce(sum(l.amount) FILTER (WHERE l.side = 'credit'), 0)",
    )
    .bind(after)
    .bind(last)
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;
    for row in &unbalanced {
        findings.push(PeriodFinding::EntryUnbalanced {
            entry: row.try_get("entry_id").map_err(db)?,
            currency: row.try_get("currency").map_err(db)?,
            debits: row.try_get("debits").map_err(db)?,
            credits: row.try_get("credits").map_err(db)?,
        });
    }
    let entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM journal_entries WHERE sequence > $1 AND sequence <= $2",
    )
    .bind(after)
    .bind(last)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;

    // The totals at the end of the period, first way: the previous closed
    // period's snapshot plus this period's lines (every line up to `last`
    // if there is no earlier snapshot).
    let previous: Option<i64> =
        sqlx::query_scalar("SELECT max(id) FROM periods WHERE status = 'closed' AND id < $1")
            .bind(period.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
    let from = if previous.is_some() { after } else { 0 };
    let rows = sqlx::query(
        "SELECT account_id, sum(debits)::bigint AS debits, sum(credits)::bigint AS credits FROM (
             SELECT account_id, debits_total AS debits, credits_total AS credits
               FROM period_snapshots WHERE period_id = $3
             UNION ALL
             SELECT l.account_id,
                    CASE l.side WHEN 'debit' THEN l.amount ELSE 0 END,
                    CASE l.side WHEN 'credit' THEN l.amount ELSE 0 END
               FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
              WHERE e.sequence > $1 AND e.sequence <= $2
         ) totals
         GROUP BY account_id ORDER BY account_id",
    )
    .bind(from)
    .bind(last)
    .bind(previous)
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;
    let mut snapshots = Vec::with_capacity(rows.len());
    for row in &rows {
        snapshots.push(Snapshot {
            account: account_id(row.try_get("account_id").map_err(db)?)?,
            debits_total: row.try_get("debits").map_err(db)?,
            credits_total: row.try_get("credits").map_err(db)?,
        });
    }

    // Second way: the stored running totals on each account's last line up
    // to `last` (ADR-0012).
    let stored_rows = sqlx::query(
        "SELECT DISTINCT ON (l.account_id) l.account_id, l.debits_after, l.credits_after
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE e.sequence <= $1
          ORDER BY l.account_id, e.sequence DESC, l.line_no DESC",
    )
    .bind(last)
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;
    let mut stored: BTreeMap<AccountId, (i64, i64)> = BTreeMap::new();
    for row in &stored_rows {
        stored.insert(
            account_id(row.try_get("account_id").map_err(db)?)?,
            (
                row.try_get("debits_after").map_err(db)?,
                row.try_get("credits_after").map_err(db)?,
            ),
        );
    }
    let computed: BTreeMap<AccountId, (i64, i64)> = snapshots
        .iter()
        .map(|s| (s.account, (s.debits_total, s.credits_total)))
        .collect();
    let accounts: BTreeSet<AccountId> = computed.keys().chain(stored.keys()).copied().collect();
    for account in accounts {
        let from_lines = computed.get(&account).copied().unwrap_or((0, 0));
        let on_lines = stored.get(&account).copied().unwrap_or((0, 0));
        if from_lines != on_lines {
            findings.push(PeriodFinding::TotalsDisagree {
                account: account.as_uuid(),
                from_lines,
                stored: on_lines,
            });
        }
    }

    if findings.is_empty() {
        let accounts: Vec<Uuid> = snapshots.iter().map(|s| s.account.as_uuid()).collect();
        let debits: Vec<i64> = snapshots.iter().map(|s| s.debits_total).collect();
        let credits: Vec<i64> = snapshots.iter().map(|s| s.credits_total).collect();
        sqlx::query(
            "INSERT INTO period_snapshots (period_id, account_id, debits_total, credits_total)
             SELECT $1, * FROM UNNEST($2::uuid[], $3::int8[], $4::int8[])",
        )
        .bind(period.id)
        .bind(&accounts)
        .bind(&debits)
        .bind(&credits)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query(
            "UPDATE periods SET status = 'closed', closed_at = clock_timestamp() WHERE id = $1",
        )
        .bind(period.id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    }
    let period = period_where(&mut tx, &format!("id = {}", period.id))
        .await?
        .ok_or_else(|| storage("the period disappeared"))?;
    let next = period_where(&mut tx, "status = 'open'").await?;
    if findings.is_empty() {
        tx.commit().await.map_err(db)?;
    } else {
        tx.rollback().await.map_err(db)?;
    }
    Ok(CloseReport {
        period,
        next,
        entries,
        snapshots: snapshots.len(),
        findings,
    })
}

/// Closes the open period and opens `next_label` (see the module
/// documentation). If a period is already closing, finishes that one
/// instead and opens nothing new.
pub async fn close_period(
    conn: &mut PgConnection,
    next_label: &str,
) -> Result<CloseReport, LedgerError> {
    begin_close(conn, next_label).await?;
    finish_close(conn).await
}

/// The snapshots of a closed period, by account.
pub async fn period_snapshots(
    conn: &mut PgConnection,
    period: i64,
) -> Result<Vec<Snapshot>, LedgerError> {
    let rows = sqlx::query(
        "SELECT account_id, debits_total, credits_total FROM period_snapshots
          WHERE period_id = $1 ORDER BY account_id",
    )
    .bind(period)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter()
        .map(|row| {
            Ok(Snapshot {
                account: account_id(row.try_get("account_id").map_err(db)?)?,
                debits_total: row.try_get("debits_total").map_err(db)?,
                credits_total: row.try_get("credits_total").map_err(db)?,
            })
        })
        .collect()
}

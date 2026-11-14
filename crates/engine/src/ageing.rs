//! Ageing of open items (step 2.02).
//!
//! Some accounts hold items that should come back to zero: one sub-account
//! per item, open while its balance is not zero. The ageing query lists
//! every open item under the account codes it is given, with how long it
//! has been open, so an item that is stuck shows up instead of sitting
//! unnoticed.
//!
//! The engine does not know which codes these are or what the ages mean.
//! The caller passes the codes, and the policy that gives them meaning
//! lives with the chart (for the wallet-provider chart,
//! `docs/accounting/clearing-and-suspense.md`).

use mezgeb_core::{Account, LedgerError, Timestamp};
use sqlx::{PgConnection, Row};

use crate::rows::{ACCOUNT_COLUMNS, account_from_row, db, storage};

/// One open item: an account with a non-zero balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenItem {
    pub account: Account,
    /// When the account was first posted to: when the item opened.
    pub since: Timestamp,
    /// `now` minus `since`, in microseconds. Never negative.
    pub age_micros: i64,
}

/// Every account opened under one of `codes` whose balance is not zero,
/// oldest first (then by account ID), with its age at `now`. `now` comes
/// from the caller's clock.
pub async fn ageing(
    conn: &mut PgConnection,
    codes: &[u16],
    now: Timestamp,
) -> Result<Vec<OpenItem>, LedgerError> {
    let codes: Vec<i16> = codes
        .iter()
        .map(|c| i16::try_from(*c).map_err(storage))
        .collect::<Result<_, _>>()?;
    let columns = ACCOUNT_COLUMNS
        .split(", ")
        .map(|c| format!("a.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {columns},
                (extract(epoch FROM opened.first_posted) * 1000000)::bigint AS since_micros
           FROM accounts a
           CROSS JOIN LATERAL (
                SELECT min(e.recorded_time) AS first_posted
                  FROM journal_lines l
                  JOIN journal_entries e ON e.id = l.entry_id
                 WHERE l.account_id = a.id
           ) opened
          WHERE a.code = ANY($1)
            AND a.debits_total <> a.credits_total
          ORDER BY opened.first_posted, a.id"
    )))
    .bind(&codes)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;

    rows.iter()
        .map(|row| {
            let account = account_from_row(row)?;
            let since: i64 = row.try_get("since_micros").map_err(db)?;
            Ok(OpenItem {
                account,
                since: Timestamp::from_micros(since),
                age_micros: now.as_micros().saturating_sub(since).max(0),
            })
        })
        .collect()
}

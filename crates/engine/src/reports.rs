//! Reports (step 2.05): the trial balance and the general ledger.
//!
//! Both are computed from the journal, so they always agree with it:
//!
//! - [`trial_balance`] lists every account's totals as of now (the stored
//!   running totals), the end of a closed period (its snapshots, which never
//!   change), or a point in time (the lines of every entry recorded by then).
//!   Every entry balances, so every trial balance balances, in every
//!   currency.
//! - [`general_ledger`] lists one account's lines between two times, with
//!   its balance before, after each line, and at the end.
//!
//! "Recorded by then" means the entry's `recorded_time`, the posting date.
//! An entry's `event_time`, its original date, does not move it.

use std::collections::BTreeMap;

use mezgeb_core::{
    Account, AccountId, AccountType, Currency, EntryId, LedgerError, Side, Timestamp,
};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};

use crate::rows::{ACCOUNT_COLUMNS, account_from_row, db, entry_id, storage};

/// Which moment a trial balance shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsOf {
    /// Everything posted so far.
    Now,
    /// The end of a closed period, from its snapshots.
    Period(i64),
    /// Every entry recorded at or before this time.
    Time(Timestamp),
}

/// One account in a trial balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialBalanceRow {
    pub account: AccountId,
    /// As written in the chart: `1010` or `2010:cust-abebe`.
    pub reference: String,
    pub account_type: AccountType,
    pub currency: Currency,
    pub debits_total: i64,
    pub credits_total: i64,
}

impl TrialBalanceRow {
    /// The balance on the account's normal side.
    pub fn balance(&self) -> i64 {
        match self.account_type.normal_side() {
            Side::Debit => self.debits_total - self.credits_total,
            Side::Credit => self.credits_total - self.debits_total,
        }
    }

    /// The balance in the debit column (zero if it is a credit balance).
    pub fn debit_balance(&self) -> i64 {
        (self.debits_total - self.credits_total).max(0)
    }

    /// The balance in the credit column (zero if it is a debit balance).
    pub fn credit_balance(&self) -> i64 {
        (self.credits_total - self.debits_total).max(0)
    }
}

/// A trial balance: every account with any postings, in chart order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialBalance {
    pub as_of: AsOf,
    pub rows: Vec<TrialBalanceRow>,
}

impl TrialBalance {
    /// The debit and credit balance columns, summed per currency.
    pub fn totals(&self) -> BTreeMap<Currency, (i64, i64)> {
        let mut totals: BTreeMap<Currency, (i64, i64)> = BTreeMap::new();
        for row in &self.rows {
            let t = totals.entry(row.currency).or_insert((0, 0));
            t.0 += row.debit_balance();
            t.1 += row.credit_balance();
        }
        totals
    }

    /// Whether the debit balances equal the credit balances in every
    /// currency. Always true for a correct ledger (INV-2).
    pub fn is_balanced(&self) -> bool {
        self.totals()
            .values()
            .all(|(debits, credits)| debits == credits)
    }

    /// The row of one account, if it has any postings.
    pub fn row(&self, account: AccountId) -> Option<&TrialBalanceRow> {
        self.rows.iter().find(|r| r.account == account)
    }
}

fn micros_param(time: Timestamp) -> i64 {
    time.as_micros()
}

/// `timestamptz` from a microsecond parameter, in SQL.
const AT: &str = "timestamptz 'epoch' + $1::bigint * interval '1 microsecond'";

fn trial_balance_row(row: &PgRow) -> Result<TrialBalanceRow, LedgerError> {
    let account = account_from_row(row)?;
    Ok(TrialBalanceRow {
        account: account.id(),
        reference: account.reference(),
        account_type: account.account_type(),
        currency: account.currency(),
        debits_total: row.try_get("debits").map_err(db)?,
        credits_total: row.try_get("credits").map_err(db)?,
    })
}

/// The trial balance as of `as_of`.
pub async fn trial_balance(
    conn: &mut PgConnection,
    as_of: AsOf,
) -> Result<TrialBalance, LedgerError> {
    let columns = ACCOUNT_COLUMNS
        .split(", ")
        .map(|c| format!("a.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let order = "ORDER BY a.code, a.sub_account NULLS FIRST";
    let rows = match as_of {
        AsOf::Now => {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {columns}, a.debits_total AS debits, a.credits_total AS credits
                   FROM accounts a
                  WHERE a.debits_total > 0 OR a.credits_total > 0 {order}"
            )))
            .fetch_all(&mut *conn)
            .await
        }
        AsOf::Period(period) => {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM periods WHERE id = $1")
                    .bind(period)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(db)?;
            if status.as_deref() != Some("closed") {
                return Err(LedgerError::PeriodNotClosed(period));
            }
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {columns}, s.debits_total AS debits, s.credits_total AS credits
                   FROM period_snapshots s JOIN accounts a ON a.id = s.account_id
                  WHERE s.period_id = $1
                    AND (s.debits_total > 0 OR s.credits_total > 0) {order}"
            )))
            .bind(period)
            .fetch_all(&mut *conn)
            .await
        }
        AsOf::Time(time) => {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {columns}, t.debits, t.credits
                   FROM accounts a
                   JOIN (SELECT l.account_id,
                                coalesce(sum(l.amount) FILTER (WHERE l.side = 'debit'), 0)::bigint
                                    AS debits,
                                coalesce(sum(l.amount) FILTER (WHERE l.side = 'credit'), 0)::bigint
                                    AS credits
                           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
                          WHERE e.recorded_time <= {AT}
                          GROUP BY l.account_id) t ON t.account_id = a.id
                  {order}"
            )))
            .bind(micros_param(time))
            .fetch_all(&mut *conn)
            .await
        }
    }
    .map_err(db)?;
    let rows = rows
        .iter()
        .map(trial_balance_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TrialBalance { as_of, rows })
}

/// One line in a general ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneralLedgerLine {
    pub entry: EntryId,
    /// The entry's posting order (INV-12).
    pub sequence: u64,
    /// When the entry was recorded: its posting date.
    pub recorded_at: Timestamp,
    pub side: Side,
    pub amount: i64,
    /// The account's balance on its normal side after this line.
    pub balance: i64,
}

/// One account's lines between two times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneralLedger {
    pub account: Account,
    pub from: Timestamp,
    pub to: Timestamp,
    /// The balance on the account's normal side from every entry recorded
    /// before `from`.
    pub opening_balance: i64,
    /// Every line of an entry recorded at or after `from` and before `to`,
    /// in posting order.
    pub lines: Vec<GeneralLedgerLine>,
    /// The opening balance plus the lines.
    pub closing_balance: i64,
}

/// The general ledger of `account` from `from` (inclusive) to `to`
/// (exclusive), by recording time.
pub async fn general_ledger(
    conn: &mut PgConnection,
    account: AccountId,
    from: Timestamp,
    to: Timestamp,
) -> Result<GeneralLedger, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1"
    )))
    .bind(account.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?
    .ok_or(LedgerError::AccountNotFound(account))?;
    let found = account_from_row(&row)?;
    let normal = found.normal_side();
    let signed = |side: Side, amount: i64| if side == normal { amount } else { -amount };

    let opening: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT coalesce(sum(CASE l.side WHEN $3 THEN l.amount ELSE -l.amount END), 0)::bigint
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE l.account_id = $2 AND e.recorded_time < {AT}"
    )))
    .bind(micros_param(from))
    .bind(account.as_uuid())
    .bind(normal.as_str())
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;

    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT e.id, e.sequence,
                (extract(epoch FROM e.recorded_time) * 1000000)::bigint AS recorded_micros,
                l.side, l.amount
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE l.account_id = $2
            AND e.recorded_time >= {AT}
            AND e.recorded_time < timestamptz 'epoch' + $3::bigint * interval '1 microsecond'
          ORDER BY e.sequence, l.line_no"
    )))
    .bind(micros_param(from))
    .bind(account.as_uuid())
    .bind(micros_param(to))
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut balance = opening;
    let mut lines = Vec::with_capacity(rows.len());
    for row in &rows {
        let side: String = row.try_get("side").map_err(db)?;
        let side = Side::from_name(&side).ok_or_else(|| storage("stored side is invalid"))?;
        let amount: i64 = row.try_get("amount").map_err(db)?;
        balance += signed(side, amount);
        let sequence: i64 = row.try_get("sequence").map_err(db)?;
        lines.push(GeneralLedgerLine {
            entry: entry_id(row.try_get("id").map_err(db)?)?,
            sequence: u64::try_from(sequence).map_err(storage)?,
            recorded_at: Timestamp::from_micros(row.try_get("recorded_micros").map_err(db)?),
            side,
            amount,
            balance,
        });
    }
    Ok(GeneralLedger {
        account: found,
        from,
        to,
        opening_balance: opening,
        lines,
        closing_balance: balance,
    })
}

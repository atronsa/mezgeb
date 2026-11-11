//! Integrity queries over the stored ledger.
//!
//! The first version of the verification job (step 1.13 extends it): checks
//! that can be run at any time, for example after a crash, to prove that no
//! partial posting was left behind.

use sqlx::PgConnection;

/// Counts of everything that would mean a corrupted ledger. All zero means
/// the checks passed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Entries with fewer than 2 lines (INV-1, INV-4).
    pub entries_without_lines: i64,
    /// Entries whose debits and credits differ in some currency (INV-1).
    pub unbalanced_entries: i64,
    /// Accounts whose stored totals differ from the sum of their lines (INV-5).
    pub accounts_with_wrong_totals: i64,
    /// Currencies in which all debits differ from all credits (INV-2).
    pub unbalanced_currencies: i64,
}

impl IntegrityReport {
    pub fn is_clean(&self) -> bool {
        *self == IntegrityReport::default()
    }
}

/// Runs every integrity query.
pub async fn integrity_report(conn: &mut PgConnection) -> Result<IntegrityReport, sqlx::Error> {
    let entries_without_lines = sqlx::query_scalar(
        "SELECT count(*) FROM journal_entries e
          WHERE (SELECT count(*) FROM journal_lines l WHERE l.entry_id = e.id) < 2",
    )
    .fetch_one(&mut *conn)
    .await?;
    let unbalanced_entries = sqlx::query_scalar(
        "SELECT count(*) FROM (
             SELECT entry_id FROM journal_lines
              GROUP BY entry_id, currency
             HAVING sum(CASE side WHEN 'debit' THEN amount ELSE -amount END) <> 0
         ) unbalanced",
    )
    .fetch_one(&mut *conn)
    .await?;
    let accounts_with_wrong_totals = sqlx::query_scalar(
        "SELECT count(*) FROM accounts a
          WHERE a.debits_total <> coalesce((SELECT sum(amount) FROM journal_lines l
                                             WHERE l.account_id = a.id AND l.side = 'debit'), 0)
             OR a.credits_total <> coalesce((SELECT sum(amount) FROM journal_lines l
                                              WHERE l.account_id = a.id AND l.side = 'credit'), 0)",
    )
    .fetch_one(&mut *conn)
    .await?;
    let unbalanced_currencies = sqlx::query_scalar(
        "SELECT count(*) FROM (
             SELECT currency FROM accounts GROUP BY currency
             HAVING sum(debits_total) <> sum(credits_total)
         ) unbalanced",
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(IntegrityReport {
        entries_without_lines,
        unbalanced_entries,
        accounts_with_wrong_totals,
        unbalanced_currencies,
    })
}

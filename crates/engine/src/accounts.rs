//! Accounts as the API needs them (step 2.07): opening an account once per
//! idempotency key, finding accounts, and an account's totals at a time.

use mezgeb_core::{
    Account, AccountCode, AccountId, Chart, ClientId, IdempotencyKey, LedgerError, Timestamp,
};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::rows::{ACCOUNT_COLUMNS, account_from_row, constraint_violation, db, storage};

/// What [`open_account_keyed`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// The account was opened by this request.
    New(Account),
    /// The same client already opened it with this key: the account as it
    /// is now.
    Repeated(Account),
}

impl Opened {
    pub fn account(&self) -> &Account {
        match self {
            Opened::New(a) | Opened::Repeated(a) => a,
        }
    }
}

/// The parts of a chart reference: `"1010"` or `"2010:cust-abebe"`.
fn split_reference(reference: &str) -> Option<(i16, Option<&str>)> {
    let (code, sub) = match reference.split_once(':') {
        Some((code, sub)) => (code, Some(sub)),
        None => (reference, None),
    };
    let code: u16 = code.parse().ok()?;
    let code = AccountCode::new(code).ok()?;
    Some((i16::try_from(code.value()).ok()?, sub))
}

async fn opened_with_key(
    conn: &mut PgConnection,
    client: ClientId,
    key: &IdempotencyKey,
) -> Result<Option<Account>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE opened_by = $1 AND open_key = $2"
    )))
    .bind(client.as_uuid())
    .bind(key.as_str())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(account_from_row).transpose()
}

/// Opens an account for `reference`, once per `(client, key)`. A repeat with
/// the same key and reference returns the account; the same key for another
/// reference is `IdempotencyKeyReused`.
pub async fn open_account_keyed(
    conn: &mut PgConnection,
    client: ClientId,
    key: &IdempotencyKey,
    id: AccountId,
    chart: &Chart,
    reference: &str,
) -> Result<Opened, LedgerError> {
    let account = Account::open(id, chart, reference).map_err(LedgerError::Chart)?;
    let same = |existing: Account| {
        if existing.reference() == account.reference() {
            Ok(Opened::Repeated(existing))
        } else {
            Err(LedgerError::IdempotencyKeyReused)
        }
    };
    if let Some(existing) = opened_with_key(conn, client, key).await? {
        return same(existing);
    }
    let inserted = sqlx::query(
        "INSERT INTO accounts
             (id, code, sub_account, name, account_type, currency, status, allow_negative,
              opened_by, open_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id.as_uuid())
    .bind(i16::try_from(account.code().value()).map_err(storage)?)
    .bind(account.sub_account())
    .bind(account.name())
    .bind(account.account_type().as_str())
    .bind(account.currency().code())
    .bind(account.status().as_str())
    .bind(account.allow_negative())
    .bind(client.as_uuid())
    .bind(key.as_str())
    .execute(&mut *conn)
    .await;
    match inserted {
        Ok(_) => Ok(Opened::New(account)),
        Err(error) => match constraint_violation(&error) {
            // A concurrent request with the same key won.
            Some((code, constraint))
                if code == "23505" && constraint == "accounts_open_key_unique" =>
            {
                let existing = opened_with_key(conn, client, key)
                    .await?
                    .ok_or_else(|| storage("an account opened with this key disappeared"))?;
                same(existing)
            }
            Some((code, constraint))
                if code == "23505" && constraint == "accounts_reference_unique" =>
            {
                Err(LedgerError::DuplicateReference(account.reference()))
            }
            Some((code, constraint)) if code == "23505" && constraint == "accounts_pkey" => {
                Err(LedgerError::DuplicateAccount(id))
            }
            _ => Err(db(error)),
        },
    }
}

/// An account, by ID.
pub async fn get_account(
    conn: &mut PgConnection,
    id: AccountId,
) -> Result<Option<Account>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1"
    )))
    .bind(id.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(account_from_row).transpose()
}

/// An account, by chart reference (`"1010"`, `"2010:cust-abebe"`). `None`
/// for a malformed reference too.
pub async fn account_by_reference(
    conn: &mut PgConnection,
    reference: &str,
) -> Result<Option<Account>, LedgerError> {
    let Some((code, sub)) = split_reference(reference) else {
        return Ok(None);
    };
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM accounts
          WHERE code = $1 AND sub_account IS NOT DISTINCT FROM $2"
    )))
    .bind(code)
    .bind(sub)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(account_from_row).transpose()
}

/// Up to `limit` accounts after `after`, by ID. With `opened_by`, only the
/// accounts that client opened.
pub async fn list_accounts(
    conn: &mut PgConnection,
    opened_by: Option<ClientId>,
    after: Option<AccountId>,
    limit: i64,
) -> Result<Vec<Account>, LedgerError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM accounts
          WHERE ($1::uuid IS NULL OR opened_by = $1)
            AND ($2::uuid IS NULL OR id > $2)
          ORDER BY id LIMIT $3"
    )))
    .bind(opened_by.map(|c| c.as_uuid()))
    .bind(after.map(|a| a.as_uuid()))
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter().map(account_from_row).collect()
}

/// An account's debit and credit totals from every entry recorded at or
/// before `time`.
pub async fn totals_at(
    conn: &mut PgConnection,
    account: AccountId,
    time: Timestamp,
) -> Result<(i64, i64), LedgerError> {
    let row = sqlx::query(
        "SELECT coalesce(sum(l.amount) FILTER (WHERE l.side = 'debit'), 0)::bigint AS debits,
                coalesce(sum(l.amount) FILTER (WHERE l.side = 'credit'), 0)::bigint AS credits
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE l.account_id = $1
            AND e.recorded_time <= timestamptz 'epoch' + $2::bigint * interval '1 microsecond'",
    )
    .bind(account.as_uuid())
    .bind(time.as_micros())
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    Ok((
        row.try_get("debits").map_err(db)?,
        row.try_get("credits").map_err(db)?,
    ))
}

/// Who opened an account, if it was opened through a client.
pub async fn account_opener(
    conn: &mut PgConnection,
    account: AccountId,
) -> Result<Option<ClientId>, LedgerError> {
    let opener: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT opened_by FROM accounts WHERE id = $1")
            .bind(account.as_uuid())
            .fetch_optional(&mut *conn)
            .await
            .map_err(db)?;
    opener
        .flatten()
        .map(|id| ClientId::from_uuid(id).map_err(storage))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::split_reference;

    #[test]
    fn references_split_into_code_and_sub_account() {
        assert_eq!(split_reference("1010"), Some((1010, None)));
        assert_eq!(
            split_reference("2010:cust-abebe"),
            Some((2010, Some("cust-abebe")))
        );
        assert_eq!(split_reference("abc"), None);
        assert_eq!(split_reference("99"), None);
        assert_eq!(split_reference("7010"), None);
    }
}

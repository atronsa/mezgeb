//! Converting between database rows and `mezgeb-core` types, and database
//! errors into ledger errors.

use std::collections::BTreeMap;

use mezgeb_core::{
    Account, AccountCode, AccountId, AccountRecord, AccountStatus, AccountType, ActorId,
    Authorization, Currency, EntryId, EntryKind, LedgerError, Money, PositiveAmount, PostedEntry,
    PostedLine, ReasonCode, Side, Timestamp,
};
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

/// The account columns, in the order [`account_from_row`] reads them.
pub(crate) const ACCOUNT_COLUMNS: &str = "id, code, sub_account, name, account_type, currency, \
     allow_negative, status, debits_total, credits_total";

pub(crate) fn storage(error: impl std::fmt::Display) -> LedgerError {
    LedgerError::Storage(error.to_string())
}

/// Maps a database error. Deadlocks and serialization failures become
/// `Contention` (the caller may retry the whole transaction); a lock wait
/// or statement longer than `lock_timeout` (`55P03`, `57014`), a
/// transaction ended by `transaction_timeout`, or no free pool connection
/// in time, becomes `Busy`.
pub(crate) fn db(error: sqlx::Error) -> LedgerError {
    if matches!(error, sqlx::Error::PoolTimedOut) {
        return LedgerError::Busy;
    }
    let code = error
        .as_database_error()
        .and_then(|e| e.code())
        .map(|c| c.into_owned());
    match code.as_deref() {
        Some(code @ ("40P01" | "40001")) => LedgerError::Contention(code.to_owned()),
        Some("55P03" | "57014" | "25P04") => LedgerError::Busy,
        _ => storage(error),
    }
}

fn bad(what: &str, value: impl std::fmt::Debug) -> LedgerError {
    LedgerError::Storage(format!("stored {what} is invalid: {value:?}"))
}

pub(crate) fn account_id(uuid: Uuid) -> Result<AccountId, LedgerError> {
    AccountId::from_uuid(uuid).map_err(|_| bad("account id", uuid))
}

pub(crate) fn entry_id(uuid: Uuid) -> Result<EntryId, LedgerError> {
    EntryId::from_uuid(uuid).map_err(|_| bad("entry id", uuid))
}

/// Rebuilds an account, refusing a row that breaks the account rules.
pub(crate) fn account_from_row(row: &PgRow) -> Result<Account, LedgerError> {
    let code: i16 = row.try_get("code").map_err(storage)?;
    let account_type: String = row.try_get("account_type").map_err(storage)?;
    let currency: String = row.try_get("currency").map_err(storage)?;
    let status: String = row.try_get("status").map_err(storage)?;
    let record = AccountRecord {
        id: account_id(row.try_get("id").map_err(storage)?)?,
        code: u16::try_from(code)
            .ok()
            .and_then(|c| AccountCode::new(c).ok())
            .ok_or_else(|| bad("account code", code))?,
        sub_account: row.try_get("sub_account").map_err(storage)?,
        name: row.try_get("name").map_err(storage)?,
        account_type: AccountType::from_name(&account_type)
            .ok_or_else(|| bad("account type", &account_type))?,
        currency: Currency::from_code(&currency).map_err(|_| bad("currency", &currency))?,
        allow_negative: row.try_get("allow_negative").map_err(storage)?,
        status: AccountStatus::from_name(&status).ok_or_else(|| bad("status", &status))?,
        debits_total: row.try_get("debits_total").map_err(storage)?,
        credits_total: row.try_get("credits_total").map_err(storage)?,
    };
    Account::from_record(record).map_err(storage)
}

/// Builds posted lines from `journal_lines` rows.
pub(crate) fn line_from_row(row: &PgRow) -> Result<PostedLine, LedgerError> {
    let side: String = row.try_get("side").map_err(storage)?;
    let currency: String = row.try_get("currency").map_err(storage)?;
    let currency = Currency::from_code(&currency).map_err(|_| bad("currency", &currency))?;
    let amount: i64 = row.try_get("amount").map_err(storage)?;
    Ok(PostedLine::new(
        account_id(row.try_get("account_id").map_err(storage)?)?,
        Side::from_name(&side).ok_or_else(|| bad("side", &side))?,
        PositiveAmount::new(Money::from_minor(amount, currency))
            .map_err(|_| bad("amount", amount))?,
        row.try_get("debits_after").map_err(storage)?,
        row.try_get("credits_after").map_err(storage)?,
    ))
}

/// Builds a posted entry from a `journal_entries` row and its lines.
pub(crate) fn entry_from_row(
    row: &PgRow,
    lines: Vec<PostedLine>,
) -> Result<PostedEntry, LedgerError> {
    let sequence: i64 = row.try_get("sequence").map_err(storage)?;
    let reverses: Option<Uuid> = row.try_get("reverses_entry_id").map_err(storage)?;
    let metadata: String = row.try_get("metadata").map_err(storage)?;
    let event_time: Option<i64> = row.try_get("event_time_micros").map_err(storage)?;
    let kind: String = row.try_get("kind").map_err(storage)?;
    let kind = EntryKind::from_name(&kind).ok_or_else(|| bad("entry kind", &kind))?;
    let reason: Option<String> = row.try_get("reason_code").map_err(storage)?;
    let requested_by: Option<String> = row.try_get("requested_by").map_err(storage)?;
    let approved_by: Option<String> = row.try_get("approved_by").map_err(storage)?;
    let authorization = match (reason, requested_by) {
        (Some(reason), Some(requested_by)) => Some(Authorization {
            reason: ReasonCode::new(reason).map_err(storage)?,
            requested_by: ActorId::new(requested_by).map_err(storage)?,
            approved_by: approved_by.map(ActorId::new).transpose().map_err(storage)?,
        }),
        _ => None,
    };
    Ok(PostedEntry::new(
        entry_id(row.try_get("id").map_err(storage)?)?,
        u64::try_from(sequence).map_err(|_| bad("sequence", sequence))?,
        lines,
        reverses.map(entry_id).transpose()?,
        serde_json::from_str::<BTreeMap<String, String>>(&metadata)
            .map_err(|_| bad("metadata", &metadata))?,
        event_time.map(Timestamp::from_micros),
        kind,
        authorization,
    ))
}

/// The SQLSTATE and constraint name of a database error, if it is one.
pub(crate) fn constraint_violation(error: &sqlx::Error) -> Option<(String, String)> {
    let db = error.as_database_error()?;
    Some((db.code()?.into_owned(), db.constraint()?.to_owned()))
}

//! What several handlers share: a connection, accounts named by ID or
//! reference, who may see or bind an account, and the answer for a posted
//! entry.
//!
//! **Ownership (ADR-0017, TM-05).** A client sees and binds only the
//! accounts it opened, plus *system accounts* (no sub-account, such as
//! `1010` or `4010`) where a template role or a hold allows them. An entry
//! is visible to the client that posted it and to the clients whose
//! accounts it touches. A caller with `accounts:read_all` (operator,
//! auditor) sees everything. Whatever is not visible is answered exactly as
//! if it did not exist, so IDs cannot be probed.

use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use mezgeb_core::{Account, AccountId, EntryId};
use mezgeb_engine::EntryRecord;

use sqlx::Postgres;
use sqlx::pool::PoolConnection;

use crate::AppState;
use crate::auth::Principal;
use crate::http::reply::written;
use crate::problem::ApiError;
use crate::wire::EntryJson;

pub(crate) type ApiResult = Result<Response, ApiError>;

pub(crate) async fn connection(state: &AppState) -> Result<PoolConnection<Postgres>, ApiError> {
    state.pool.acquire().await.map_err(|e| match e {
        sqlx::Error::PoolTimedOut => {
            ApiError::new("busy", "No database connection was free in time.")
        }
        other => ApiError::internal(other),
    })
}

/// An account named by ID or by chart reference.
pub(crate) async fn resolve_account(
    conn: &mut sqlx::PgConnection,
    text: &str,
) -> Result<Option<AccountId>, ApiError> {
    if let Ok(id) = text.parse::<AccountId>() {
        return Ok(Some(id));
    }
    Ok(mezgeb_engine::account_by_reference(conn, text)
        .await?
        .map(|a| a.id()))
}

/// Whether the caller opened this account.
pub(crate) async fn owns(
    conn: &mut sqlx::PgConnection,
    caller: &Principal,
    account: AccountId,
) -> Result<bool, ApiError> {
    Ok(mezgeb_engine::account_opener(conn, account).await? == Some(caller.client))
}

/// The account, if the caller may see it: its own, or any with
/// `accounts:read_all`. Otherwise `404 not_found`, as for no account.
pub(crate) async fn visible_account(
    conn: &mut sqlx::PgConnection,
    caller: &Principal,
    id: AccountId,
) -> Result<Account, ApiError> {
    let account = mezgeb_engine::get_account(conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    if caller.sees_all() || owns(conn, caller, id).await? {
        Ok(account)
    } else {
        Err(ApiError::not_found("account"))
    }
}

/// An account named by ID or reference that the caller may bind in a
/// posting or a hold: one it opened, or a system account. `None`, exactly
/// as for no such account, otherwise.
pub(crate) async fn bindable_account(
    conn: &mut sqlx::PgConnection,
    caller: &Principal,
    text: &str,
) -> Result<Option<AccountId>, ApiError> {
    let Some(id) = resolve_account(conn, text).await? else {
        return Ok(None);
    };
    if owns(conn, caller, id).await? {
        return Ok(Some(id));
    }
    let account = mezgeb_engine::get_account(conn, id).await?;
    Ok(account
        .filter(|a| a.sub_account().is_none())
        .map(|a| a.id()))
}

/// Whether the caller may see an entry: it posted it, it touches one of
/// the caller's accounts, or the caller sees everything.
pub(crate) async fn entry_visible(
    conn: &mut sqlx::PgConnection,
    caller: &Principal,
    record: &EntryRecord,
) -> Result<bool, ApiError> {
    if caller.sees_all() || record.client == caller.client {
        return Ok(true);
    }
    for line in record.entry.lines() {
        if owns(conn, caller, line.account()).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) async fn entry_response(
    conn: &mut sqlx::PgConnection,
    id: EntryId,
) -> Result<EntryRecord, ApiError> {
    mezgeb_engine::entry_record(conn, id)
        .await?
        .ok_or_else(|| ApiError::internal("a posted entry is missing"))
}

pub(crate) fn new_entry_id() -> Result<EntryId, ApiError> {
    EntryId::from_uuid(uuid::Uuid::now_v7()).map_err(ApiError::internal)
}

pub(crate) fn entry_written(record: &EntryRecord, new: bool) -> Response {
    let mut response = written(new, EntryJson::from(record), StatusCode::CREATED);
    if let Ok(location) = HeaderValue::from_str(&format!("/v1/entries/{}", record.entry.id())) {
        response.headers_mut().insert("location", location);
    }
    response
}

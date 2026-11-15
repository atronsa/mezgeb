//! What several handlers share: a connection, accounts named by ID or
//! reference, and the answer for a posted entry.

use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use mezgeb_core::{AccountId, EntryId};
use mezgeb_engine::EntryRecord;

use sqlx::Postgres;
use sqlx::pool::PoolConnection;

use crate::AppState;
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

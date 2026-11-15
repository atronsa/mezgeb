//! Entries: get one, or find one by its idempotency key.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use mezgeb_core::EntryId;

use super::common::{ApiResult, connection};
use crate::AppState;
use crate::http::extract::Caller;
use crate::http::reply::json_response;
use crate::problem::ApiError;
use crate::wire::{EntryJson, parse_id};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/entries/{entry_id}", get(get_entry))
        .route(
            "/v1/entries/by-idempotency-key/{key}",
            get(get_entry_by_key),
        )
}

async fn get_entry(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(entry_id): Path<String>,
) -> ApiResult {
    caller.require("entries:read")?;
    let id: EntryId = parse_id(&entry_id, "entry")?;
    let mut conn = connection(&state).await?;
    let record = mezgeb_engine::entry_record(&mut conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("entry"))?;
    Ok(json_response(StatusCode::OK, EntryJson::from(&record)))
}

async fn get_entry_by_key(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(key): Path<String>,
) -> ApiResult {
    caller.require("entries:read")?;
    let key = mezgeb_core::IdempotencyKey::new(key).map_err(|_| ApiError::not_found("entry"))?;
    let mut conn = connection(&state).await?;
    let record = mezgeb_engine::entry_record_by_key(&mut conn, caller.client, &key)
        .await?
        .ok_or_else(|| ApiError::not_found("entry"))?;
    Ok(json_response(StatusCode::OK, EntryJson::from(&record)))
}

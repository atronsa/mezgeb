//! Audit: the signing keys, the signed checkpoints and the audit events
//! (ADR-0018).

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use serde::Deserialize;
use serde_json::json;

use super::common::{ApiResult, connection};
use crate::AppState;
use crate::auth::scope::{AuditRead, Auth, EntriesRead};
use crate::http::extract::QueryParams;
use crate::http::pagination::{Page, limit};
use crate::http::reply::json_response;
use crate::problem::ApiError;
use crate::wire::{AuditEventJson, CheckpointJson, SigningKeyJson};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/keys", get(list_keys))
        .route("/v1/checkpoints", get(list_checkpoints))
        .route("/v1/audit-events", get(list_audit_events))
}

fn cursor(text: Option<&str>) -> Result<u64, ApiError> {
    text.map_or(Ok(0), |c| {
        c.parse()
            .map_err(|_| ApiError::invalid("/query/cursor", "unknown cursor"))
    })
}

async fn list_keys(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<EntriesRead>,
) -> ApiResult {
    let mut conn = connection(&state).await?;
    let keys = mezgeb_engine::signing_keys(&mut conn).await?;
    let items: Vec<SigningKeyJson> = keys
        .into_iter()
        .map(|(key_id, public)| SigningKeyJson {
            key_id,
            algorithm: "Ed25519",
            public_key: mezgeb_core::chain::hex(&public),
        })
        .collect();
    Ok(json_response(StatusCode::OK, json!({ "items": items })))
}

async fn list_checkpoints(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<EntriesRead>,
    QueryParams(page): QueryParams<Page>,
) -> ApiResult {
    let limit = usize::try_from(limit(page.limit)?).unwrap_or(50);
    let after = i64::try_from(cursor(page.cursor.as_deref())?).unwrap_or(i64::MAX);
    let mut conn = connection(&state).await?;
    let all = mezgeb_engine::checkpoints(&mut conn).await?;
    let rest: Vec<_> = all.iter().filter(|c| c.id > after).collect();
    let next = (rest.len() > limit).then(|| rest[limit - 1].id.to_string());
    let items: Vec<CheckpointJson> = rest
        .into_iter()
        .take(limit)
        .map(CheckpointJson::from)
        .collect();
    Ok(json_response(
        StatusCode::OK,
        json!({ "items": items, "next_cursor": next }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditQuery {
    client: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_audit_events(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<AuditRead>,
    QueryParams(query): QueryParams<AuditQuery>,
) -> ApiResult {
    let limit = limit(query.limit)?;
    let after = cursor(query.cursor.as_deref())?;
    let client = query
        .client
        .map(|c| {
            c.parse()
                .map_err(|_| ApiError::invalid("/query/client", "not a client ID"))
        })
        .transpose()?;
    let mut conn = connection(&state).await?;
    let events = mezgeb_engine::audit_events(&mut conn, client, after, limit + 1).await?;
    let shown = usize::try_from(limit).unwrap_or(0);
    let next = (events.len() > shown).then(|| events[shown - 1].position.to_string());
    let items: Vec<AuditEventJson> = events
        .iter()
        .take(shown)
        .map(AuditEventJson::from)
        .collect();
    Ok(json_response(
        StatusCode::OK,
        json!({ "items": items, "next_cursor": next }),
    ))
}

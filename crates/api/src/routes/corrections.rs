//! Corrections: reversals, and maker-checker adjustments.

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::routing::{get, post};
use mezgeb_core::{Authorization, EntryDraft, EntryId, LineDraft, ReasonCode};
use mezgeb_engine::{NewAdjustment, Outcome, PostRequest};
use serde::Deserialize;

use super::common::{
    ApiResult, connection, entry_response, entry_written, new_entry_id, resolve_account,
};
use crate::AppState;
use crate::http::extract::{Caller, IdemKey, JsonBody, parse_id};
use crate::http::reply::json_response;
use crate::problem::ApiError;
use crate::wire::{AdjustmentJson, MoneyJson, parse_side};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/entries/{entry_id}/reversal", post(reverse_entry))
        .route("/v1/adjustments", post(request_adjustment))
        .route("/v1/adjustments/{adjustment_id}", get(get_adjustment))
        .route(
            "/v1/adjustments/{adjustment_id}/approval",
            post(approve_adjustment),
        )
        .route(
            "/v1/adjustments/{adjustment_id}/rejection",
            post(reject_adjustment),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReversalBody {
    reason: String,
    #[allow(dead_code)]
    note: Option<String>,
}

async fn reverse_entry(
    State(state): State<AppState>,
    Caller(caller): Caller,
    IdemKey(key): IdemKey,
    Path(entry_id): Path<String>,
    JsonBody(body): JsonBody<ReversalBody>,
) -> ApiResult {
    caller.require("corrections:write")?;
    let original: EntryId = parse_id(&entry_id, "entry")?;
    let reason =
        ReasonCode::new(&body.reason).map_err(|e| ApiError::invalid("/reason", e.to_string()))?;
    let id = new_entry_id()?;
    let authorization = Authorization {
        reason,
        requested_by: caller.actor.clone(),
        approved_by: None,
    };
    let record = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let request = PostRequest::new(id, caller.client, key, EntryDraft::default())
                    .authorized(authorization);
                let posted =
                    mezgeb_engine::reverse_entry_with(&mut conn, original, request, &state.options)
                        .await?;
                entry_response(&mut conn, posted.id()).await
            }
        })
        .await?;
    Ok(entry_written(&record, record.entry.id() == id))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdjustmentLineBody {
    account: String,
    side: String,
    amount: MoneyJson,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdjustmentBody {
    reason: String,
    lines: Vec<AdjustmentLineBody>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    note: Option<String>,
}

async fn request_adjustment(
    State(state): State<AppState>,
    Caller(caller): Caller,
    IdemKey(key): IdemKey,
    JsonBody(body): JsonBody<AdjustmentBody>,
) -> ApiResult {
    caller.require("adjustments:request")?;
    let reason =
        ReasonCode::new(&body.reason).map_err(|e| ApiError::invalid("/reason", e.to_string()))?;
    if body.lines.len() > 100 {
        return Err(ApiError::invalid("/lines", "at most 100 lines"));
    }
    let mut conn = connection(&state).await?;
    let mut lines = Vec::with_capacity(body.lines.len());
    for (n, line) in body.lines.iter().enumerate() {
        let pointer = format!("/lines/{n}");
        let account = resolve_account(&mut conn, &line.account)
            .await?
            .ok_or_else(|| {
                ApiError::invalid(
                    &format!("{pointer}/account"),
                    format!("no account {:?}", line.account),
                )
            })?;
        lines.push(LineDraft {
            account,
            side: parse_side(&line.side, &format!("{pointer}/side"))?,
            amount: line.amount.to_money(&format!("{pointer}/amount"))?,
        });
    }
    drop(conn);
    let request = NewAdjustment {
        reason,
        lines,
        metadata: body.metadata,
        note: body.note,
        requested_by: caller.actor.clone(),
    };
    let id = uuid::Uuid::now_v7();
    let outcome = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(
                    mezgeb_engine::request_adjustment(&mut conn, id, caller.client, &key, &request)
                        .await?,
                )
            }
        })
        .await?;
    // A repeat is also 202: the request is pending either way.
    let mut response = json_response(StatusCode::ACCEPTED, AdjustmentJson::from(outcome.value()));
    if matches!(outcome, Outcome::Repeated(_)) {
        response
            .headers_mut()
            .insert("idempotent-replayed", HeaderValue::from_static("true"));
    }
    Ok(response)
}

async fn get_adjustment(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(adjustment_id): Path<String>,
) -> ApiResult {
    caller.require("adjustments:request")?;
    let id: uuid::Uuid = parse_id(&adjustment_id, "adjustment request")?;
    let mut conn = connection(&state).await?;
    let request = mezgeb_engine::get_adjustment(&mut conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("adjustment request"))?;
    Ok(json_response(
        StatusCode::OK,
        AdjustmentJson::from(&request),
    ))
}

async fn approve_adjustment(
    State(state): State<AppState>,
    Caller(caller): Caller,
    IdemKey(_key): IdemKey,
    Path(adjustment_id): Path<String>,
) -> ApiResult {
    caller.require("adjustments:approve")?;
    let id: uuid::Uuid = parse_id(&adjustment_id, "adjustment request")?;
    let (record, new) = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let outcome =
                    mezgeb_engine::approve_adjustment(&mut conn, id, &caller.actor, &state.options)
                        .await?;
                let new = matches!(outcome, Outcome::Done(_));
                let (_, posted) = outcome.into_value();
                Ok((entry_response(&mut conn, posted.id()).await?, new))
            }
        })
        .await?;
    Ok(entry_written(&record, new))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectionBody {
    note: String,
}

async fn reject_adjustment(
    State(state): State<AppState>,
    Caller(caller): Caller,
    IdemKey(_key): IdemKey,
    Path(adjustment_id): Path<String>,
    JsonBody(body): JsonBody<RejectionBody>,
) -> ApiResult {
    caller.require("adjustments:approve")?;
    let id: uuid::Uuid = parse_id(&adjustment_id, "adjustment request")?;
    if body.note.is_empty() || body.note.len() > 500 {
        return Err(ApiError::invalid(
            "/note",
            "the note must be 1 to 500 characters",
        ));
    }
    let request = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(
                    mezgeb_engine::reject_adjustment(&mut conn, id, &caller.actor, &body.note)
                        .await?
                        .into_value(),
                )
            }
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        AdjustmentJson::from(&request),
    ))
}

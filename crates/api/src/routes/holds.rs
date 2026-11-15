//! Holds: place, get, capture, release.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::routing::{get, post};
use mezgeb_core::{HoldId, Money, PositiveAmount};
use mezgeb_engine::{CaptureHold, HoldRequest, PlaceHold};
use serde::Deserialize;

use super::common::{ApiResult, bindable_account, connection, new_entry_id, owns};
use crate::AppState;
use crate::auth::Principal;
use crate::auth::scope::{Auth, HoldsRead, HoldsWrite};
use crate::http::extract::{IdemKey, JsonBody, parse_id};
use crate::http::reply::{json_response, written};
use crate::problem::ApiError;
use crate::wire::{HoldJson, MoneyJson, now, parse_time};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/holds", post(place_hold))
        .route("/v1/holds/{hold_id}", get(get_hold))
        .route("/v1/holds/{hold_id}/capture", post(capture_hold))
        .route("/v1/holds/{hold_id}/release", post(release_hold))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaceHoldBody {
    account: String,
    held_account: String,
    amount: MoneyJson,
    expires_at: String,
}

fn positive(money: Money, pointer: &str) -> Result<PositiveAmount, ApiError> {
    PositiveAmount::new(money)
        .map_err(|_| ApiError::invalid(pointer, "the amount must be greater than zero"))
}

async fn place_hold(
    State(state): State<AppState>,
    Auth(caller, _): Auth<HoldsWrite>,
    IdemKey(key): IdemKey,
    JsonBody(body): JsonBody<PlaceHoldBody>,
) -> ApiResult {
    let amount = positive(body.amount.to_money("/amount")?, "/amount/amount")?;
    let expires_at = parse_time(&body.expires_at, "/expires_at")?;
    let mut conn = connection(&state).await?;
    let missing = |field: &str, text: &str| {
        ApiError::new("account_not_found", format!("No account {text:?}."))
            .with_name(field.to_owned())
    };
    let account = bindable_account(&mut conn, &caller, &body.account)
        .await?
        .ok_or_else(|| missing("account", &body.account))?;
    let held = bindable_account(&mut conn, &caller, &body.held_account)
        .await?
        .ok_or_else(|| missing("held_account", &body.held_account))?;
    drop(conn);
    let entry = new_entry_id()?;
    let hold_id = HoldId::from_uuid(uuid::Uuid::now_v7()).map_err(ApiError::internal)?;
    let (hold, new) = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                // A hold's request names its hold ID, which a retry cannot
                // know: a repeated key returns the hold it placed.
                if let Some(record) =
                    mezgeb_engine::entry_record_by_key(&mut conn, caller.client, &key).await?
                {
                    return match mezgeb_engine::hold_of(&mut conn, record.entry.id()).await? {
                        Some(hold)
                            if hold.place_entry() == record.entry.id()
                                && hold.account() == account
                                && hold.held_account() == held
                                && hold.amount() == amount
                                && hold.expires_at() == expires_at =>
                        {
                            Ok((hold, false))
                        }
                        _ => Err(ApiError::from(
                            mezgeb_core::LedgerError::IdempotencyKeyReused,
                        )),
                    };
                }
                let request = HoldRequest {
                    entry_id: entry,
                    client_id: caller.client,
                    idempotency_key: key,
                };
                let place = PlaceHold {
                    hold: hold_id,
                    account,
                    held,
                    amount,
                    expires_at,
                };
                let hold =
                    mezgeb_engine::place_hold(&mut conn, &request, place, &state.options).await?;
                Ok((hold, true))
            }
        })
        .await?;
    let mut response = written(new, HoldJson::from(&hold), StatusCode::CREATED);
    if let Ok(location) = HeaderValue::from_str(&format!("/v1/holds/{}", hold.id())) {
        response.headers_mut().insert("location", location);
    }
    Ok(response)
}

/// A hold on one of the caller's accounts (or any, with
/// `accounts:read_all`); otherwise `404`, as for no hold.
async fn own_hold(
    conn: &mut sqlx::PgConnection,
    caller: &Principal,
    id: HoldId,
) -> Result<mezgeb_core::Hold, ApiError> {
    let hold = mezgeb_engine::get_hold(conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("hold"))?;
    if caller.sees_all() || owns(conn, caller, hold.account()).await? {
        Ok(hold)
    } else {
        Err(ApiError::not_found("hold"))
    }
}

/// A hold the caller may capture or release: one on its own account.
/// `accounts:read_all` reads holds but never moves another client's.
async fn movable_hold(
    conn: &mut sqlx::PgConnection,
    caller: &Principal,
    id: HoldId,
) -> Result<(), ApiError> {
    let hold = mezgeb_engine::get_hold(conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("hold"))?;
    if owns(conn, caller, hold.account()).await? {
        Ok(())
    } else {
        Err(ApiError::not_found("hold"))
    }
}

async fn get_hold(
    State(state): State<AppState>,
    Auth(caller, _): Auth<HoldsRead>,
    Path(hold_id): Path<String>,
) -> ApiResult {
    let id: HoldId = parse_id(&hold_id, "hold")?;
    let mut conn = connection(&state).await?;
    let hold = own_hold(&mut conn, &caller, id).await?;
    Ok(json_response(StatusCode::OK, HoldJson::from(&hold)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureBody {
    destination: String,
    amount: MoneyJson,
}

async fn capture_hold(
    State(state): State<AppState>,
    Auth(caller, _): Auth<HoldsWrite>,
    IdemKey(key): IdemKey,
    Path(hold_id): Path<String>,
    JsonBody(body): JsonBody<CaptureBody>,
) -> ApiResult {
    let hold: HoldId = parse_id(&hold_id, "hold")?;
    let amount = positive(body.amount.to_money("/amount")?, "/amount/amount")?;
    let mut conn = connection(&state).await?;
    movable_hold(&mut conn, &caller, hold).await?;
    let destination = bindable_account(&mut conn, &caller, &body.destination)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                "account_not_found",
                format!("No account {:?}.", body.destination),
            )
        })?;
    drop(conn);
    let entry = new_entry_id()?;
    let captured = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let request = HoldRequest {
                    entry_id: entry,
                    client_id: caller.client,
                    idempotency_key: key,
                };
                let capture = CaptureHold {
                    hold,
                    destination,
                    amount,
                    now: now(),
                };
                Ok(
                    mezgeb_engine::capture_hold(&mut conn, &request, capture, &state.options)
                        .await?,
                )
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, HoldJson::from(&captured)))
}

async fn release_hold(
    State(state): State<AppState>,
    Auth(caller, _): Auth<HoldsWrite>,
    IdemKey(key): IdemKey,
    Path(hold_id): Path<String>,
) -> ApiResult {
    let hold: HoldId = parse_id(&hold_id, "hold")?;
    let mut conn = connection(&state).await?;
    movable_hold(&mut conn, &caller, hold).await?;
    drop(conn);
    let entry = new_entry_id()?;
    let released = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let request = HoldRequest {
                    entry_id: entry,
                    client_id: caller.client,
                    idempotency_key: key,
                };
                Ok(mezgeb_engine::release_hold(&mut conn, &request, hold, &state.options).await?)
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, HoldJson::from(&released)))
}

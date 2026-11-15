//! Periods: list, the open one, one by ID, close.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;

use super::common::{ApiResult, connection};
use crate::AppState;
use crate::auth::scope::{Auth, PeriodsClose, PeriodsRead};
use crate::http::extract::{JsonBody, parse_id};
use crate::http::pagination::{Page, limit};
use crate::http::reply::json_response;
use crate::problem::ApiError;
use crate::wire::PeriodJson;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/periods", get(list_periods))
        .route("/v1/periods/current", get(current_period))
        .route("/v1/periods/close", post(close_period))
        .route("/v1/periods/{period_id}", get(get_period))
}

async fn list_periods(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<PeriodsRead>,
    Query(page): Query<Page>,
) -> ApiResult {
    let limit = usize::try_from(limit(page.limit)?).unwrap_or(50);
    let before = page
        .cursor
        .map(|c| {
            c.parse::<i64>()
                .map_err(|_| ApiError::invalid("/query/cursor", "unknown cursor"))
        })
        .transpose()?;
    let mut conn = connection(&state).await?;
    let mut periods = mezgeb_engine::all_periods(&mut conn).await?;
    periods.reverse();
    let periods: Vec<_> = periods
        .into_iter()
        .filter(|p| before.is_none_or(|b| p.id < b))
        .collect();
    let next = (periods.len() > limit).then(|| periods[limit - 1].id.to_string());
    let items: Vec<PeriodJson> = periods.iter().take(limit).map(PeriodJson::from).collect();
    Ok(json_response(
        StatusCode::OK,
        json!({"items": items, "next_cursor": next}),
    ))
}

async fn current_period(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<PeriodsRead>,
) -> ApiResult {
    let mut conn = connection(&state).await?;
    let period = mezgeb_engine::current_period(&mut conn)
        .await?
        .ok_or_else(|| ApiError::from(mezgeb_core::LedgerError::NoOpenPeriod))?;
    Ok(json_response(StatusCode::OK, PeriodJson::from(&period)))
}

async fn get_period(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<PeriodsRead>,
    Path(period_id): Path<String>,
) -> ApiResult {
    let id: i64 = parse_id(&period_id, "period")?;
    let mut conn = connection(&state).await?;
    let period = mezgeb_engine::all_periods(&mut conn)
        .await?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::not_found("period"))?;
    Ok(json_response(StatusCode::OK, PeriodJson::from(&period)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseBody {
    next_label: String,
}

async fn close_period(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<PeriodsClose>,
    JsonBody(body): JsonBody<CloseBody>,
) -> ApiResult {
    let label_ok = !body.next_label.is_empty()
        && body.next_label.len() <= 64
        && body
            .next_label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b));
    if !label_ok {
        return Err(ApiError::invalid(
            "/next_label",
            "1 to 64 characters from A-Z a-z 0-9 . _ : -",
        ));
    }
    let report = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let periods = mezgeb_engine::all_periods(&mut conn).await?;
                let resuming = periods
                    .iter()
                    .any(|p| p.status == mezgeb_engine::PeriodStatus::Closing);
                if !resuming && periods.iter().any(|p| p.label == body.next_label) {
                    return Err(ApiError::new(
                        "period_label_taken",
                        format!("A period is already labelled {:?}.", body.next_label),
                    ));
                }
                Ok(mezgeb_engine::close_period(&mut conn, &body.next_label).await?)
            }
        })
        .await?;
    if !report.is_closed() {
        let findings: Vec<String> = report.findings.iter().map(ToString::to_string).collect();
        return Err(ApiError::new(
            "period_close_findings",
            format!(
                "Period {} stays closing: {}",
                report.period.id,
                findings.join("; ")
            ),
        ));
    }
    let next = report
        .next
        .as_ref()
        .ok_or_else(|| ApiError::internal("no open period after a close"))?;
    Ok(json_response(
        StatusCode::OK,
        json!({
            "period": PeriodJson::from(&report.period),
            "next": PeriodJson::from(next),
            "entries": report.entries,
            "snapshots": report.snapshots,
        }),
    ))
}

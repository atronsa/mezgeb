//! Reports: trial balance and ageing.

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use mezgeb_core::Money;
use mezgeb_engine::AsOf;
use mezgeb_ops::AGED_ACCOUNTS;
use serde::Deserialize;
use serde_json::{Value, json};

use super::common::{ApiResult, connection};
use crate::AppState;
use crate::auth::scope::{Auth, ReportsRead};
use crate::http::reply::json_response;
use crate::problem::ApiError;
use crate::wire::{MoneyJson, format_time, now, parse_time};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/reports/trial-balance", get(trial_balance))
        .route("/v1/reports/ageing", get(ageing))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrialBalanceQuery {
    period_id: Option<i64>,
    as_of: Option<String>,
}

async fn trial_balance(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<ReportsRead>,
    Query(query): Query<TrialBalanceQuery>,
) -> ApiResult {
    let as_of = match (query.period_id, &query.as_of) {
        (Some(_), Some(_)) => {
            return Err(ApiError::invalid(
                "/query",
                "give period_id or as_of, not both",
            ));
        }
        (Some(period), None) => AsOf::Period(period),
        (None, Some(text)) => AsOf::Time(parse_time(text, "/query/as_of")?),
        (None, None) => AsOf::Now,
    };
    let mut conn = connection(&state).await?;
    let tb = mezgeb_engine::trial_balance(&mut conn, as_of).await?;
    let rows: Vec<Value> = tb
        .rows
        .iter()
        .map(|r| {
            let money = |minor: i64| MoneyJson::from(Money::from_minor(minor, r.currency));
            json!({
                "account": r.account.to_string(),
                "reference": r.reference,
                "type": r.account_type.as_str(),
                "debits_total": money(r.debits_total),
                "credits_total": money(r.credits_total),
                "debit_balance": money(r.debit_balance()),
                "credit_balance": money(r.credit_balance()),
            })
        })
        .collect();
    let totals: Vec<Value> = tb
        .totals()
        .iter()
        .map(|(currency, (debits, credits))| {
            json!({
                "debit_balances": MoneyJson::from(Money::from_minor(*debits, *currency)),
                "credit_balances": MoneyJson::from(Money::from_minor(*credits, *currency)),
            })
        })
        .collect();
    let (period_id, time) = match as_of {
        AsOf::Period(p) => (Some(p), None),
        AsOf::Time(t) => (None, Some(format_time(t))),
        AsOf::Now => (None, None),
    };
    Ok(json_response(
        StatusCode::OK,
        json!({
            "as_of": {"period_id": period_id, "time": time},
            "rows": rows,
            "totals": totals,
            "balanced": tb.is_balanced(),
        }),
    ))
}

async fn ageing(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<ReportsRead>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    let mut codes = Vec::new();
    for pair in raw.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key != "code" {
            return Err(ApiError::invalid(
                "/query",
                format!("unknown parameter {key:?}"),
            ));
        }
        let code: u16 = value
            .parse()
            .ok()
            .filter(|c| (1000..=9999).contains(c))
            .ok_or_else(|| {
                ApiError::invalid("/query/code", format!("{value:?} is not an account code"))
            })?;
        codes.push(code);
    }
    if codes.len() > 20 {
        return Err(ApiError::invalid("/query/code", "at most 20 codes"));
    }
    if codes.is_empty() {
        codes = AGED_ACCOUNTS.to_vec();
    }
    let mut conn = connection(&state).await?;
    let items = mezgeb_engine::ageing(&mut conn, &codes, now()).await?;
    let body: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "account": i.account.id().to_string(),
                "reference": i.account.reference(),
                "balance": MoneyJson::from(i.account.balance()),
                "since": format_time(i.since),
                "age_seconds": i.age_micros / 1_000_000,
            })
        })
        .collect();
    Ok(json_response(StatusCode::OK, body))
}

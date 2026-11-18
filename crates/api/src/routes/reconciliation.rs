//! Reconciliation (step 2.15, ADR-0020).
//!
//! - `POST /v1/reconciliation/balances`: a client compares its cached
//!   balances with the ledger's, for its own accounts. The ledger wins: a
//!   disagreement is a `stale_cache` break, and the answer carries the
//!   ledger's balance to refresh from.
//! - `GET /v1/reconciliation/breaks`, `GET …/breaks/{id}`,
//!   `GET /v1/reconciliation/runs`: what reconciliation found.
//! - `POST …/breaks/{id}/resolve`: an operator resolves a money break with
//!   the suspense entry that put the account right.
//! - `POST …/breaks/{id}/explain`: an operator closes another break with a
//!   reason.
//!
//! Statement import and the daily runs are `mezgeb-cli recon` commands.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use mezgeb_core::{AccountId, EntryId};
use mezgeb_engine::recon::{
    BreakFilter, BreakKind, BreakStatus, CachedBalance, ReconError, RunKind, explain_break,
    get_break, list_breaks, list_runs, reconcile_cache, resolve_break,
};
use serde::Deserialize;
use serde_json::json;

use super::common::{ApiResult, connection, resolve_account, visible_account};
use crate::AppState;
use crate::auth::scope::{Auth, ReconciliationRead, ReconciliationResolve, ReconciliationWrite};
use crate::http::extract::{JsonBody, QueryParams, parse_id};
use crate::http::reply::json_response;
use crate::problem::ApiError;
use crate::wire::{CacheResultJson, MoneyJson, ReconBreakJson, ReconRunJson, now};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/reconciliation/balances", post(compare_balances))
        .route("/v1/reconciliation/breaks", get(breaks))
        .route("/v1/reconciliation/breaks/{break_id}", get(one_break))
        .route(
            "/v1/reconciliation/breaks/{break_id}/resolve",
            post(resolve),
        )
        .route(
            "/v1/reconciliation/breaks/{break_id}/explain",
            post(explain),
        )
        .route("/v1/reconciliation/runs", get(runs))
}

impl From<ReconError> for ApiError {
    fn from(error: ReconError) -> ApiError {
        let detail = error.to_string();
        match error {
            ReconError::Invalid(_) => ApiError::new("invalid_request", detail),
            ReconError::NotFound(what) => {
                ApiError::not_found(what.split(' ').next().unwrap_or("break"))
            }
            ReconError::Closed(_) => ApiError::new("break_closed", detail),
            ReconError::Conflict(_) => ApiError::new("break_not_closable", detail),
            ReconError::Ledger(e) => ApiError::from(e),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedBalanceBody {
    account: String,
    balance: MoneyJson,
    as_of_sequence: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BalancesBody {
    balances: Vec<CachedBalanceBody>,
}

/// Compares the caller's cached balances with the ledger's.
async fn compare_balances(
    State(state): State<AppState>,
    Auth(caller, _): Auth<ReconciliationWrite>,
    JsonBody(body): JsonBody<BalancesBody>,
) -> ApiResult {
    if body.balances.is_empty() || body.balances.len() > 1000 {
        return Err(ApiError::invalid("/balances", "send 1 to 1,000 balances"));
    }
    let mut conn = connection(&state).await?;
    let mut balances = Vec::with_capacity(body.balances.len());
    let mut currencies = Vec::with_capacity(body.balances.len());
    for (i, b) in body.balances.iter().enumerate() {
        // The caller's own accounts only: any other is not found.
        let id = resolve_account(&mut conn, &b.account)
            .await?
            .ok_or_else(|| ApiError::not_found("account"))?;
        let account = visible_account(&mut conn, &caller, id).await?;
        let money = b.balance.to_money(&format!("/balances/{i}/balance"))?;
        if money.currency() != account.currency() {
            return Err(ApiError::invalid(
                &format!("/balances/{i}/balance/currency"),
                format!("the account is in {}", account.currency().code()),
            ));
        }
        balances.push(CachedBalance {
            account: id,
            balance_minor: money.amount(),
            as_of_sequence: b.as_of_sequence,
        });
        currencies.push(account.currency());
    }
    drop(conn);
    let client = caller.client;
    let report = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(reconcile_cache(&mut conn, client, &balances, now()).await?)
            }
        })
        .await?;
    let results: Vec<CacheResultJson> = report
        .results
        .iter()
        .zip(currencies)
        .map(|(r, currency)| CacheResultJson::new(r, currency))
        .collect();
    Ok(json_response(
        StatusCode::OK,
        json!({ "run": ReconRunJson::from(&report.run), "results": results }),
    ))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BreakQuery {
    status: Option<String>,
    kind: Option<String>,
    account: Option<String>,
    after: Option<String>,
    limit: Option<i64>,
}

/// Breaks, oldest first; open ones unless `status` says otherwise.
async fn breaks(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<ReconciliationRead>,
    QueryParams(query): QueryParams<BreakQuery>,
) -> ApiResult {
    let status = match query.status.as_deref() {
        None => Some(BreakStatus::Open),
        Some("all") => None,
        Some(s) => Some(s.parse::<BreakStatus>().map_err(|_| {
            ApiError::invalid("/status", "open, cleared, resolved, explained or all")
        })?),
    };
    let kind = query
        .kind
        .as_deref()
        .map(str::parse::<BreakKind>)
        .transpose()
        .map_err(|e| ApiError::invalid("/kind", e))?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=1000).contains(&limit) {
        return Err(ApiError::invalid("/limit", "the limit is 1 to 1,000"));
    }
    let after = query
        .after
        .as_deref()
        .map(|a| a.parse::<uuid::Uuid>())
        .transpose()
        .map_err(|_| ApiError::invalid("/after", "a break ID"))?;
    let mut conn = connection(&state).await?;
    let account: Option<AccountId> = match query.account.as_deref() {
        None => None,
        Some(text) => Some(
            resolve_account(&mut conn, text)
                .await?
                .ok_or_else(|| ApiError::invalid("/account", "no such account"))?,
        ),
    };
    let found = list_breaks(
        &mut conn,
        &BreakFilter {
            status,
            kind,
            account,
            client: None,
            after,
            limit,
        },
    )
    .await?;
    let next = (found.len() == usize::try_from(limit).unwrap_or(usize::MAX))
        .then(|| found.last().map(|b| b.id.to_string()))
        .flatten();
    let items: Vec<ReconBreakJson> = found.iter().map(ReconBreakJson::from).collect();
    Ok(json_response(
        StatusCode::OK,
        json!({ "items": items, "next_after": next }),
    ))
}

async fn one_break(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<ReconciliationRead>,
    Path(break_id): Path<String>,
) -> ApiResult {
    let id: uuid::Uuid = parse_id(&break_id, "break")?;
    let mut conn = connection(&state).await?;
    let found = get_break(&mut conn, id).await?;
    Ok(json_response(StatusCode::OK, ReconBreakJson::from(&found)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    entry_id: String,
}

/// Resolves a money break with the suspense entry that put the account
/// right (`deposit_unmatched.v1` or `bank_debit_unmatched.v1`).
async fn resolve(
    State(state): State<AppState>,
    Auth(caller, _): Auth<ReconciliationResolve>,
    Path(break_id): Path<String>,
    JsonBody(body): JsonBody<ResolveBody>,
) -> ApiResult {
    let id: uuid::Uuid = parse_id(&break_id, "break")?;
    let entry: EntryId = body
        .entry_id
        .parse()
        .map_err(|_| ApiError::invalid("/entry_id", "an entry ID"))?;
    let client = caller.client;
    let resolved = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(resolve_break(
                    &mut conn,
                    id,
                    entry,
                    client,
                    mezgeb_ops::RECON_RESOLUTION_TEMPLATES,
                    now(),
                )
                .await?)
            }
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        ReconBreakJson::from(&resolved),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplainBody {
    reason: String,
}

/// Closes a break that is not about money with the operator's reason.
async fn explain(
    State(state): State<AppState>,
    Auth(caller, _): Auth<ReconciliationResolve>,
    Path(break_id): Path<String>,
    JsonBody(body): JsonBody<ExplainBody>,
) -> ApiResult {
    let id: uuid::Uuid = parse_id(&break_id, "break")?;
    if body.reason.trim().is_empty() || body.reason.chars().count() > 1000 {
        return Err(ApiError::invalid(
            "/reason",
            "the reason is 1 to 1,000 characters",
        ));
    }
    let client = caller.client;
    let explained = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(explain_break(&mut conn, id, &body.reason, client, now()).await?)
            }
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        ReconBreakJson::from(&explained),
    ))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RunQuery {
    kind: Option<String>,
    limit: Option<i64>,
}

/// The latest runs, newest first.
async fn runs(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<ReconciliationRead>,
    QueryParams(query): QueryParams<RunQuery>,
) -> ApiResult {
    let kind = query
        .kind
        .as_deref()
        .map(str::parse::<RunKind>)
        .transpose()
        .map_err(|e| ApiError::invalid("/kind", e))?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=1000).contains(&limit) {
        return Err(ApiError::invalid("/limit", "the limit is 1 to 1,000"));
    }
    let mut conn = connection(&state).await?;
    let found = list_runs(&mut conn, kind, limit).await?;
    let items: Vec<ReconRunJson> = found.iter().map(ReconRunJson::from).collect();
    Ok(json_response(StatusCode::OK, json!({ "items": items })))
}

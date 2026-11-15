//! Accounts: open, get, list or find, balance, history, status.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::routing::{get, post};
use mezgeb_core::{AccountId, AccountStatus, Ledger, Money, ReasonCode};
use mezgeb_engine::{LedgerCursor, Opened, PgStore};
use serde::Deserialize;
use serde_json::{Value, json};

use super::common::{ApiResult, connection, owns, visible_account};
use crate::AppState;
use crate::auth::Principal;
use crate::auth::scope::{AccountsAdmin, AccountsRead, AccountsWrite, Auth};
use crate::http::extract::{IdemKey, JsonBody, parse_id};
use crate::http::pagination::limit;
use crate::http::reply::{json_response, written};
use crate::problem::ApiError;
use crate::wire::{AccountJson, MoneyJson, format_time, now, parse_time};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/accounts", post(open_account).get(list_accounts))
        .route("/v1/accounts/{account_id}", get(get_account))
        .route("/v1/accounts/{account_id}/balance", get(get_balance))
        .route("/v1/accounts/{account_id}/ledger", get(get_ledger))
        .route("/v1/accounts/{account_id}/status", post(change_status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAccountBody {
    reference: String,
}

async fn open_account(
    State(state): State<AppState>,
    Auth(caller, _): Auth<AccountsWrite>,
    IdemKey(key): IdemKey,
    JsonBody(body): JsonBody<OpenAccountBody>,
) -> ApiResult {
    let id = AccountId::from_uuid(uuid::Uuid::now_v7()).map_err(ApiError::internal)?;
    // A system account (no sub-account, such as 1010) can be bound by every
    // client's postings, so only the operator's admin opens one. A reference
    // the chart refuses is left to the engine, which answers with its code.
    let system = mezgeb_core::Account::open(id, &state.chart, &body.reference)
        .is_ok_and(|a| a.sub_account().is_none());
    if system && !caller.has("accounts:admin") {
        return Err(ApiError::new(
            "forbidden",
            "Opening a system account needs the admin role (accounts:admin).",
        ));
    }
    let chart = state.chart.clone();
    let opened = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(mezgeb_engine::open_account_keyed(
                    &mut conn,
                    caller.client,
                    &key,
                    id,
                    &chart,
                    &body.reference,
                )
                .await?)
            }
        })
        .await?;
    let new = matches!(opened, Opened::New(_));
    let mut response = written(
        new,
        AccountJson::from(opened.account()),
        StatusCode::CREATED,
    );
    if let Ok(location) = HeaderValue::from_str(&format!("/v1/accounts/{}", opened.account().id()))
    {
        response.headers_mut().insert("location", location);
    }
    Ok(response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAccountsQuery {
    reference: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_accounts(
    State(state): State<AppState>,
    Auth(caller, _): Auth<AccountsRead>,
    Query(query): Query<ListAccountsQuery>,
) -> ApiResult {
    let limit = limit(query.limit)?;
    let mut conn = connection(&state).await?;
    let opened_by = (!caller.sees_all()).then_some(caller.client);
    if let Some(reference) = query.reference {
        let found = match mezgeb_engine::account_by_reference(&mut conn, &reference).await? {
            Some(a) if caller.sees_all() || owns(&mut conn, &caller, a.id()).await? => Some(a),
            _ => None,
        };
        let items: Vec<AccountJson> = found.iter().map(AccountJson::from).collect();
        return Ok(json_response(
            StatusCode::OK,
            json!({"items": items, "next_cursor": null}),
        ));
    }
    let after = query
        .cursor
        .map(|c| {
            c.parse::<AccountId>()
                .map_err(|_| ApiError::invalid("/query/cursor", "unknown cursor"))
        })
        .transpose()?;
    let accounts = mezgeb_engine::list_accounts(&mut conn, opened_by, after, limit + 1).await?;
    let more = accounts.len() > usize::try_from(limit).unwrap_or(0);
    let page: Vec<&mezgeb_core::Account> = accounts
        .iter()
        .take(usize::try_from(limit).unwrap_or(0))
        .collect();
    let next = if more {
        page.last().map(|a| a.id().to_string())
    } else {
        None
    };
    let items: Vec<AccountJson> = page.into_iter().map(AccountJson::from).collect();
    Ok(json_response(
        StatusCode::OK,
        json!({"items": items, "next_cursor": next}),
    ))
}

/// An account the caller may see, from its path ID.
async fn load_account(
    state: &AppState,
    caller: &Principal,
    id: &str,
) -> Result<mezgeb_core::Account, ApiError> {
    let id: AccountId = parse_id(id, "account")?;
    let mut conn = connection(state).await?;
    visible_account(&mut conn, caller, id).await
}

async fn get_account(
    State(state): State<AppState>,
    Auth(caller, _): Auth<AccountsRead>,
    Path(account_id): Path<String>,
) -> ApiResult {
    let account = load_account(&state, &caller, &account_id).await?;
    Ok(json_response(StatusCode::OK, AccountJson::from(&account)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BalanceQuery {
    as_of: Option<String>,
}

async fn get_balance(
    State(state): State<AppState>,
    Auth(caller, _): Auth<AccountsRead>,
    Path(account_id): Path<String>,
    Query(query): Query<BalanceQuery>,
) -> ApiResult {
    let account = load_account(&state, &caller, &account_id).await?;
    let as_of = match &query.as_of {
        Some(text) => parse_time(text, "/query/as_of")?,
        None => now(),
    };
    let (debits, credits) = if query.as_of.is_some() {
        let mut conn = connection(&state).await?;
        mezgeb_engine::totals_at(&mut conn, account.id(), as_of).await?
    } else {
        (account.debits_total(), account.credits_total())
    };
    let currency = account.currency();
    let balance = match mezgeb_core::balance(account.normal_side(), debits, credits) {
        Some(b) => b,
        None => return Err(ApiError::internal("balance overflow")),
    };
    Ok(json_response(
        StatusCode::OK,
        json!({
            "account": account.id().to_string(),
            "as_of": format_time(as_of),
            "balance": MoneyJson::from(Money::from_minor(balance, currency)),
            "debits_total": MoneyJson::from(Money::from_minor(debits, currency)),
            "credits_total": MoneyJson::from(Money::from_minor(credits, currency)),
        }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerQuery {
    from: String,
    to: String,
    limit: Option<i64>,
    cursor: Option<String>,
}

fn parse_cursor(text: &str) -> Result<LedgerCursor, ApiError> {
    let bad = || ApiError::invalid("/query/cursor", "unknown cursor");
    let (sequence, line) = text.split_once('.').ok_or_else(bad)?;
    Ok(LedgerCursor {
        sequence: sequence.parse().map_err(|_| bad())?,
        line_no: line.parse().map_err(|_| bad())?,
    })
}

async fn get_ledger(
    State(state): State<AppState>,
    Auth(caller, _): Auth<AccountsRead>,
    Path(account_id): Path<String>,
    Query(query): Query<LedgerQuery>,
) -> ApiResult {
    let account = load_account(&state, &caller, &account_id).await?;
    let from = parse_time(&query.from, "/query/from")?;
    let to = parse_time(&query.to, "/query/to")?;
    let limit = limit(query.limit)?;
    let after = query.cursor.as_deref().map(parse_cursor).transpose()?;
    let mut conn = connection(&state).await?;
    let page =
        mezgeb_engine::general_ledger_page(&mut conn, &account, from, to, after, limit).await?;
    let currency = account.currency();
    let money = |minor: i64| MoneyJson::from(Money::from_minor(minor, currency));
    let items: Vec<Value> = page
        .lines
        .iter()
        .map(|(line, _)| {
            json!({
                "entry": line.entry.to_string(),
                "sequence": line.sequence,
                "recorded_at": format_time(line.recorded_at),
                "side": line.side.as_str(),
                "amount": money(line.amount),
                "balance": money(line.balance),
            })
        })
        .collect();
    let next = page.next.map(|c| format!("{}.{}", c.sequence, c.line_no));
    Ok(json_response(
        StatusCode::OK,
        json!({
            "account": account.id().to_string(),
            "opening_balance": money(page.opening_balance),
            "items": items,
            "next_cursor": next,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusBody {
    status: String,
    reason: String,
}

async fn change_status(
    State(state): State<AppState>,
    Auth(caller, _): Auth<AccountsAdmin>,
    IdemKey(_key): IdemKey,
    Path(account_id): Path<String>,
    JsonBody(body): JsonBody<StatusBody>,
) -> ApiResult {
    let id: AccountId = parse_id(&account_id, "account")?;
    let to = AccountStatus::from_name(&body.status)
        .ok_or_else(|| ApiError::invalid("/status", "status must be open, frozen or closed"))?;
    ReasonCode::new(&body.reason).map_err(|e| ApiError::invalid("/reason", e.to_string()))?;
    let account = state
        .write({
            let state = state.clone();
            async move {
                let mut store =
                    PgStore::with_options(state.pool.clone(), caller.client, state.options.clone());
                let current = store
                    .get_account(id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("account"))?;
                // The same status again is a repeat, not an error.
                if current.status() != to {
                    store.set_status(id, to).await?;
                }
                store
                    .get_account(id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("account"))
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, AccountJson::from(&account)))
}

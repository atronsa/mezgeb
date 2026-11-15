//! The handlers, one per operation of `openapi/mezgeb.v1.yaml`. Each one
//! authenticates the caller and checks the operation's scope before it reads
//! the body or touches the ledger.

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use mezgeb_core::{
    AccountId, AccountStatus, Authorization, EntryDraft, EntryId, HoldId, Ledger, LineDraft, Money,
    PositiveAmount, ReasonCode,
};
use mezgeb_engine::{
    AsOf, CaptureHold, EntryRecord, HoldRequest, LedgerCursor, NewAdjustment, Opened, Outcome,
    PgStore, PlaceHold, PostRequest,
};
use mezgeb_ops::{AGED_ACCOUNTS, OperationRequest};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Postgres;
use sqlx::pool::PoolConnection;

use crate::auth::Principal;
use crate::problem::ApiError;
use crate::wire::{
    AccountJson, AdjustmentJson, EntryJson, HoldJson, IdemKey, JsonBody, MoneyJson, PeriodJson,
    format_time, now, parse_id, parse_side, parse_time,
};
use crate::{AppState, LoggedClient};

type ApiResult = Result<Response, ApiError>;

pub fn routes() -> Router<AppState> {
    let mut router = Router::new()
        .route("/v1/accounts", post(open_account).get(list_accounts))
        .route("/v1/accounts/{account_id}", get(get_account))
        .route("/v1/accounts/{account_id}/balance", get(get_balance))
        .route("/v1/accounts/{account_id}/ledger", get(get_ledger))
        .route("/v1/accounts/{account_id}/status", post(change_status))
        .route("/v1/templates", get(list_templates))
        .route("/v1/templates/{template_id}", get(get_template))
        .route("/v1/entries/{entry_id}", get(get_entry))
        .route(
            "/v1/entries/by-idempotency-key/{key}",
            get(get_entry_by_key),
        )
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
        .route("/v1/holds", post(place_hold))
        .route("/v1/holds/{hold_id}", get(get_hold))
        .route("/v1/holds/{hold_id}/capture", post(capture_hold))
        .route("/v1/holds/{hold_id}/release", post(release_hold))
        .route("/v1/periods", get(list_periods))
        .route("/v1/periods/current", get(current_period))
        .route("/v1/periods/close", post(close_period))
        .route("/v1/periods/{period_id}", get(get_period))
        .route("/v1/reports/trial-balance", get(trial_balance))
        .route("/v1/reports/ageing", get(ageing))
        .route("/v1/offline/devices", post(offline))
        .route("/v1/offline/vouchers", post(offline))
        .route("/v1/offline/receipts", post(offline));
    // One route per released template version.
    for id in mezgeb_ops::RELEASED {
        router = router.route(&format!("/v1/operations/{id}"), post(operation));
    }
    router
}

// ---------------------------------------------------------------- helpers

/// The authenticated caller.
pub struct Caller(pub Principal);

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Caller, ApiError> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let principal = state.tokens.authenticate(header)?;
        if let Some(logged) = parts.extensions.get::<LoggedClient>()
            && let Ok(mut slot) = logged.0.lock()
        {
            *slot = Some(principal.client);
        }
        Ok(Caller(principal))
    }
}

async fn connection(state: &AppState) -> Result<PoolConnection<Postgres>, ApiError> {
    state.pool.acquire().await.map_err(|e| match e {
        sqlx::Error::PoolTimedOut => {
            ApiError::new("busy", "No database connection was free in time.")
        }
        other => ApiError::internal(other),
    })
}

fn json_response(status: StatusCode, body: impl serde::Serialize) -> Response {
    (status, axum::Json(body)).into_response()
}

/// `201` for a new result, `200` with `Idempotent-Replayed: true` for a
/// repeat of an earlier request.
fn written(new: bool, body: impl serde::Serialize, created: StatusCode) -> Response {
    let mut response = json_response(if new { created } else { StatusCode::OK }, body);
    if !new {
        response
            .headers_mut()
            .insert("idempotent-replayed", HeaderValue::from_static("true"));
    }
    response
}

/// An account named by ID or by chart reference.
async fn resolve_account(
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

async fn entry_response(
    conn: &mut sqlx::PgConnection,
    id: EntryId,
) -> Result<EntryRecord, ApiError> {
    mezgeb_engine::entry_record(conn, id)
        .await?
        .ok_or_else(|| ApiError::internal("a posted entry is missing"))
}

fn new_entry_id() -> Result<EntryId, ApiError> {
    EntryId::from_uuid(uuid::Uuid::now_v7()).map_err(ApiError::internal)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    limit: Option<i64>,
    cursor: Option<String>,
}

fn limit(page_limit: Option<i64>) -> Result<i64, ApiError> {
    match page_limit {
        None => Ok(50),
        Some(n) if (1..=100).contains(&n) => Ok(n),
        Some(_) => Err(ApiError::invalid(
            "/query/limit",
            "limit must be from 1 to 100",
        )),
    }
}

// ---------------------------------------------------------------- accounts

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAccountBody {
    reference: String,
}

async fn open_account(
    State(state): State<AppState>,
    Caller(caller): Caller,
    IdemKey(key): IdemKey,
    JsonBody(body): JsonBody<OpenAccountBody>,
) -> ApiResult {
    caller.require("accounts:write")?;
    let id = AccountId::from_uuid(uuid::Uuid::now_v7()).map_err(ApiError::internal)?;
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
    Caller(caller): Caller,
    Query(query): Query<ListAccountsQuery>,
) -> ApiResult {
    caller.require("accounts:read")?;
    let limit = limit(query.limit)?;
    let mut conn = connection(&state).await?;
    let opened_by = (!caller.has("accounts:read_all")).then_some(caller.client);
    if let Some(reference) = query.reference {
        let found = mezgeb_engine::account_by_reference(&mut conn, &reference).await?;
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

async fn load_account(state: &AppState, id: &str) -> Result<mezgeb_core::Account, ApiError> {
    let id: AccountId = parse_id(id, "account")?;
    let mut conn = connection(state).await?;
    mezgeb_engine::get_account(&mut conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))
}

async fn get_account(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(account_id): Path<String>,
) -> ApiResult {
    caller.require("accounts:read")?;
    let account = load_account(&state, &account_id).await?;
    Ok(json_response(StatusCode::OK, AccountJson::from(&account)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BalanceQuery {
    as_of: Option<String>,
}

async fn get_balance(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(account_id): Path<String>,
    Query(query): Query<BalanceQuery>,
) -> ApiResult {
    caller.require("accounts:read")?;
    let account = load_account(&state, &account_id).await?;
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
    Caller(caller): Caller,
    Path(account_id): Path<String>,
    Query(query): Query<LedgerQuery>,
) -> ApiResult {
    caller.require("accounts:read")?;
    let account = load_account(&state, &account_id).await?;
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
    Caller(caller): Caller,
    IdemKey(_key): IdemKey,
    Path(account_id): Path<String>,
    JsonBody(body): JsonBody<StatusBody>,
) -> ApiResult {
    caller.require("accounts:admin")?;
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

// ---------------------------------------------------------------- templates

fn template_json(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let registry = &state.registry;
    let roles: Vec<Value> = registry
        .roles(id)?
        .iter()
        .map(|r| json!({"name": r.name, "codes": r.codes, "optional": r.optional}))
        .collect();
    let metadata: Vec<Value> = registry
        .metadata(id)?
        .iter()
        .map(|m| json!({"name": m.name, "required": m.required, "values": m.values}))
        .collect();
    Ok(json!({
        "id": id,
        "roles": roles,
        "amounts": registry.amounts(id)?,
        "optional_amounts": registry.optional_amounts(id)?,
        "metadata": metadata,
        "refunds": registry.refunds(id)?,
    }))
}

async fn list_templates(State(state): State<AppState>, Caller(caller): Caller) -> ApiResult {
    caller.require("operations:write")?;
    let ids: Vec<String> = state.registry.ids().map(ToString::to_string).collect();
    let templates = ids
        .iter()
        .map(|id| template_json(&state, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json_response(StatusCode::OK, templates))
}

async fn get_template(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(template_id): Path<String>,
) -> ApiResult {
    caller.require("operations:write")?;
    Ok(json_response(
        StatusCode::OK,
        template_json(&state, &template_id)?,
    ))
}

// ---------------------------------------------------------------- operations

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationBody {
    accounts: BTreeMap<String, String>,
    amounts: BTreeMap<String, MoneyJson>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    event_time: Option<String>,
    refunds: Option<String>,
}

async fn operation(
    State(state): State<AppState>,
    Caller(caller): Caller,
    IdemKey(key): IdemKey,
    matched: axum::extract::MatchedPath,
    JsonBody(body): JsonBody<OperationBody>,
) -> ApiResult {
    caller.require("operations:write")?;
    let template = matched
        .as_str()
        .strip_prefix("/v1/operations/")
        .unwrap_or_default()
        .to_owned();
    let mut amounts = BTreeMap::new();
    for (name, money) in &body.amounts {
        amounts.insert(name.clone(), money.to_money(&format!("/amounts/{name}"))?);
    }
    let event_time = body
        .event_time
        .as_deref()
        .map(|t| parse_time(t, "/event_time"))
        .transpose()?;
    let refunds = body
        .refunds
        .as_deref()
        .map(|r| {
            r.parse::<EntryId>()
                .map_err(|_| ApiError::invalid("/refunds", "not an entry ID"))
        })
        .transpose()?;
    let mut conn = connection(&state).await?;
    let mut accounts = BTreeMap::new();
    let mut view: BTreeMap<AccountId, mezgeb_core::Account> = BTreeMap::new();
    for (role, text) in &body.accounts {
        let Some(id) = resolve_account(&mut conn, text).await? else {
            return Err(ApiError::new(
                "bound_account_not_found",
                format!("No account {text:?} for role {role:?}."),
            )
            .with_role(role));
        };
        if let Some(account) = mezgeb_engine::get_account(&mut conn, id).await? {
            view.insert(id, account);
        }
        accounts.insert(role.clone(), id);
    }
    let request = OperationRequest {
        template,
        accounts,
        amounts,
        metadata: body.metadata,
        event_time,
        refunds,
    };
    let draft = state.registry.draft(&request, &view)?;
    drop(conn);
    post_draft(&state, caller, key, draft).await
}

/// Posts a draft as the caller, idempotently, and answers with the entry.
async fn post_draft(
    state: &AppState,
    caller: Principal,
    key: mezgeb_core::IdempotencyKey,
    draft: EntryDraft,
) -> ApiResult {
    let entry_id = new_entry_id()?;
    let record = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let request = PostRequest::new(entry_id, caller.client, key, draft);
                let posted =
                    mezgeb_engine::post_entry_with(&mut conn, request, &state.options).await?;
                entry_response(&mut conn, posted.id()).await
            }
        })
        .await?;
    Ok(entry_written(&record, record.entry.id() == entry_id))
}

fn entry_written(record: &EntryRecord, new: bool) -> Response {
    let mut response = written(new, EntryJson::from(record), StatusCode::CREATED);
    if let Ok(location) = HeaderValue::from_str(&format!("/v1/entries/{}", record.entry.id())) {
        response.headers_mut().insert("location", location);
    }
    response
}

// ---------------------------------------------------------------- entries

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

// ---------------------------------------------------------------- corrections

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

// ---------------------------------------------------------------- holds

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
    Caller(caller): Caller,
    IdemKey(key): IdemKey,
    JsonBody(body): JsonBody<PlaceHoldBody>,
) -> ApiResult {
    caller.require("holds:write")?;
    let amount = positive(body.amount.to_money("/amount")?, "/amount/amount")?;
    let expires_at = parse_time(&body.expires_at, "/expires_at")?;
    let mut conn = connection(&state).await?;
    let missing = |field: &str, text: &str| {
        ApiError::new("account_not_found", format!("No account {text:?}."))
            .with_name(field.to_owned())
    };
    let account = resolve_account(&mut conn, &body.account)
        .await?
        .ok_or_else(|| missing("account", &body.account))?;
    let held = resolve_account(&mut conn, &body.held_account)
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

async fn get_hold(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(hold_id): Path<String>,
) -> ApiResult {
    caller.require("holds:read")?;
    let id: HoldId = parse_id(&hold_id, "hold")?;
    let mut conn = connection(&state).await?;
    let hold = mezgeb_engine::get_hold(&mut conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("hold"))?;
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
    Caller(caller): Caller,
    IdemKey(key): IdemKey,
    Path(hold_id): Path<String>,
    JsonBody(body): JsonBody<CaptureBody>,
) -> ApiResult {
    caller.require("holds:write")?;
    let hold: HoldId = parse_id(&hold_id, "hold")?;
    let amount = positive(body.amount.to_money("/amount")?, "/amount/amount")?;
    let mut conn = connection(&state).await?;
    let destination = resolve_account(&mut conn, &body.destination)
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
    Caller(caller): Caller,
    IdemKey(key): IdemKey,
    Path(hold_id): Path<String>,
) -> ApiResult {
    caller.require("holds:write")?;
    let hold: HoldId = parse_id(&hold_id, "hold")?;
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

// ---------------------------------------------------------------- periods

async fn list_periods(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Query(page): Query<Page>,
) -> ApiResult {
    caller.require("periods:read")?;
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

async fn current_period(State(state): State<AppState>, Caller(caller): Caller) -> ApiResult {
    caller.require("periods:read")?;
    let mut conn = connection(&state).await?;
    let period = mezgeb_engine::current_period(&mut conn)
        .await?
        .ok_or_else(|| ApiError::from(mezgeb_core::LedgerError::NoOpenPeriod))?;
    Ok(json_response(StatusCode::OK, PeriodJson::from(&period)))
}

async fn get_period(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Path(period_id): Path<String>,
) -> ApiResult {
    caller.require("periods:read")?;
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
    Caller(caller): Caller,
    JsonBody(body): JsonBody<CloseBody>,
) -> ApiResult {
    caller.require("periods:close")?;
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

// ---------------------------------------------------------------- reports

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrialBalanceQuery {
    period_id: Option<i64>,
    as_of: Option<String>,
}

async fn trial_balance(
    State(state): State<AppState>,
    Caller(caller): Caller,
    Query(query): Query<TrialBalanceQuery>,
) -> ApiResult {
    caller.require("reports:read")?;
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
    Caller(caller): Caller,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    caller.require("reports:read")?;
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

// ---------------------------------------------------------------- offline

async fn offline(Caller(caller): Caller, _headers: HeaderMap) -> ApiResult {
    caller.require("offline:write")?;
    Err(ApiError::new(
        "not_implemented",
        "Offline payments are planned for steps 2.11 and 2.12.",
    ))
}

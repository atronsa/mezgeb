//! Operations: one `POST /v1/operations/{template}` per released template.

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::State;
use axum::routing::post;
use mezgeb_core::{AccountId, EntryDraft, EntryId};
use mezgeb_engine::PostRequest;
use mezgeb_ops::OperationRequest;
use serde::Deserialize;

use super::common::{
    ApiResult, connection, entry_response, entry_written, new_entry_id, resolve_account,
};
use crate::AppState;
use crate::auth::Principal;
use crate::http::extract::{Caller, IdemKey, JsonBody};
use crate::problem::ApiError;
use crate::wire::{MoneyJson, parse_time};

pub(super) fn router() -> Router<AppState> {
    let mut router = Router::new();
    // One route per released template version.
    for id in mezgeb_ops::RELEASED {
        router = router.route(&format!("/v1/operations/{id}"), post(operation));
    }
    router
}

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

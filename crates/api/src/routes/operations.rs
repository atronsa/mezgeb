//! Operations: one `POST /v1/operations/{template}` per released template.

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::{FromRequestParts, MatchedPath, State};
use axum::http::request::Parts;
use axum::routing::post;
use mezgeb_core::{AccountId, EntryDraft, EntryId};
use mezgeb_engine::PostRequest;
use mezgeb_ops::OperationRequest;
use serde::Deserialize;

use super::common::{
    ApiResult, bindable_account, connection, entry_response, entry_written, new_entry_id,
};
use crate::AppState;
use crate::auth::Principal;
use crate::auth::scope::{Auth, OperationsWrite};
use crate::http::extract::{IdemKey, JsonBody};
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

/// The route's template, if the caller's roles may post it (the permission
/// matrix). Checked right after the scope, before the request is read.
struct Postable(String);

impl FromRequestParts<AppState> for Postable {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Postable, ApiError> {
        let template = parts
            .extensions
            .get::<MatchedPath>()
            .and_then(|p| p.as_str().strip_prefix("/v1/operations/"))
            .unwrap_or_default()
            .to_owned();
        let caller = parts
            .extensions
            .get::<Principal>()
            .ok_or_else(ApiError::unauthenticated)?;
        if caller.templates.contains(&template) {
            Ok(Postable(template))
        } else {
            Err(ApiError::new(
                "forbidden",
                format!("This credential's roles may not post {template}."),
            ))
        }
    }
}

async fn operation(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OperationsWrite>,
    Postable(template): Postable,
    IdemKey(key): IdemKey,
    JsonBody(body): JsonBody<OperationBody>,
) -> ApiResult {
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
        let Some(id) = bindable_account(&mut conn, &caller, text).await? else {
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

//! Templates: the posting template catalogue.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use serde_json::{Value, json};

use super::common::ApiResult;
use crate::AppState;
use crate::auth::scope::{Auth, OperationsWrite};
use crate::http::reply::json_response;
use crate::problem::ApiError;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/templates", get(list_templates))
        .route("/v1/templates/{template_id}", get(get_template))
}

/// A template a client may post. Those only the engine books are not in
/// the catalogue, as if unknown.
fn template_json(state: &AppState, id: &str) -> Result<Value, ApiError> {
    if mezgeb_ops::ENGINE_ONLY.contains(&id) {
        return Err(mezgeb_ops::TemplateError::UnknownTemplate(id.to_owned()).into());
    }
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

async fn list_templates(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<OperationsWrite>,
) -> ApiResult {
    let ids: Vec<String> = state
        .registry
        .ids()
        .map(ToString::to_string)
        .filter(|id| !mezgeb_ops::ENGINE_ONLY.contains(&id.as_str()))
        .collect();
    let templates = ids
        .iter()
        .map(|id| template_json(&state, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json_response(StatusCode::OK, templates))
}

async fn get_template(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<OperationsWrite>,
    Path(template_id): Path<String>,
) -> ApiResult {
    Ok(json_response(
        StatusCode::OK,
        template_json(&state, &template_id)?,
    ))
}

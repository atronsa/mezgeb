//! Offline: devices, vouchers, receipts. `501` until steps 2.11 and 2.12.

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::post;

use super::common::ApiResult;
use crate::AppState;
use crate::http::extract::Caller;
use crate::problem::ApiError;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/offline/devices", post(offline))
        .route("/v1/offline/vouchers", post(offline))
        .route("/v1/offline/receipts", post(offline))
}

async fn offline(Caller(caller): Caller, _headers: HeaderMap) -> ApiResult {
    caller.require("offline:write")?;
    Err(ApiError::new(
        "not_implemented",
        "Offline payments are planned for steps 2.11 and 2.12.",
    ))
}

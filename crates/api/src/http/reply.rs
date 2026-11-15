//! Building successful answers.

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

pub(crate) fn json_response(status: StatusCode, body: impl serde::Serialize) -> Response {
    (status, axum::Json(body)).into_response()
}

/// `201` for a new result, `200` with `Idempotent-Replayed: true` for a
/// repeat of an earlier request.
pub(crate) fn written(new: bool, body: impl serde::Serialize, created: StatusCode) -> Response {
    let mut response = json_response(if new { created } else { StatusCode::OK }, body);
    if !new {
        response
            .headers_mut()
            .insert("idempotent-replayed", HeaderValue::from_static("true"));
    }
    response
}

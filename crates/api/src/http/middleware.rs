//! What every request goes through: a request ID, the timeout, and one
//! access log line (see the crate documentation).

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use mezgeb_core::ClientId;

use crate::AppState;
use crate::problem::{ApiError, REQUEST_ID};

/// The client a request was made by, filled in by authentication, for the
/// access log.
#[derive(Clone, Default)]
pub struct LoggedClient(pub Arc<Mutex<Option<ClientId>>>);

fn caller_request_id(value: &HeaderValue) -> Option<String> {
    // A caller's ID is kept only if it is a UUID: it becomes the problem's
    // `instance`, `urn:uuid:<id>`, and goes into the log as it is.
    let text = value.to_str().ok()?;
    uuid::Uuid::parse_str(text)
        .ok()
        .map(|id| id.hyphenated().to_string())
}

pub(crate) async fn request_context(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let id = request
        .headers()
        .get("x-request-id")
        .and_then(caller_request_id)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let logged = LoggedClient::default();
    request.extensions_mut().insert(logged.clone());
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "(no route)".to_owned(), |p| p.as_str().to_owned());
    let started = Instant::now();
    let timeout = state.request_timeout;
    let response = REQUEST_ID
        .scope(id.clone(), async move {
            match tokio::time::timeout(timeout, next.run(request)).await {
                Ok(response) => response,
                Err(_) => ApiError::new(
                    "busy",
                    "The request did not finish in time. A write may still take effect: \
                     retry with the same Idempotency-Key.",
                )
                .into_response(),
            }
        })
        .await;
    let mut response = response;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", value);
    }
    let client = logged.0.lock().ok().and_then(|c| *c);
    tracing::info!(
        target: "access",
        method = %method,
        route = %route,
        status = response.status().as_u16(),
        latency_ms = started.elapsed().as_millis() as u64,
        request_id = %id,
        client = client.map(|c| c.to_string()).unwrap_or_default(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_callers_request_id_is_kept_only_if_it_is_a_uuid() {
        let id = |text: &str| caller_request_id(&HeaderValue::from_str(text).ok()?);
        assert_eq!(
            id("0192F3C4-0000-7000-8000-00000000ABCD").as_deref(),
            Some("0192f3c4-0000-7000-8000-00000000abcd")
        );
        assert_eq!(id("abc"), None);
        assert_eq!(id("0192f3c4-0000-7000-8000-00000000abcd\nforged"), None);
        assert_eq!(id(""), None);
    }
}

//! Ledger REST service (ADR-0009, spec §9, step 2.07).
//!
//! The contract is `openapi/mezgeb.v1.yaml`; this crate implements it. A
//! request goes through:
//!
//! 1. **request ID**: the client's `X-Request-Id` if it is a plain token,
//!    otherwise a new UUIDv7; returned on every response and used as each
//!    error's `instance`;
//! 2. **access log**: one line per request with the method, the route
//!    pattern (never a raw path with IDs in it), the status, the latency,
//!    the request ID and the client. Never a body, a header or a token;
//! 3. **timeout**: a request that takes longer than the limit is answered
//!    `503 busy`. A write is not cancelled by it (see below), so the client
//!    retries with the same `Idempotency-Key` and gets the one result;
//! 4. **body limit**: a larger body is `400 invalid_request`;
//! 5. the handler, which authenticates the caller, checks the scope
//!    (default deny), and calls the templates (`mezgeb-ops`) and the engine
//!    (`mezgeb-engine`).
//!
//! **Writes run to completion.** A handler runs each write in a task of its
//! own, tracked by the service. If the client goes away or the request
//! times out, the task still finishes its transaction, committed or rolled
//! back, never cut off part-way; graceful shutdown waits for every one.

pub mod auth;
pub mod config;
mod http;
pub mod problem;
mod routes;
pub mod wire;

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{DefaultBodyLimit, MatchedPath, Request, State};
use axum::http::HeaderValue;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use mezgeb_core::{Chart, ClientId};
use mezgeb_engine::PostOptions;
use mezgeb_ops::TemplateRegistry;
use sqlx::PgPool;
use tokio_util::task::TaskTracker;

use crate::auth::Tokens;
use crate::problem::{ApiError, REQUEST_ID};

/// Everything a handler needs.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub chart: Arc<Chart>,
    pub tokens: Arc<Tokens>,
    pub registry: Arc<TemplateRegistry>,
    pub options: PostOptions,
    /// The writes under way, which graceful shutdown waits for.
    pub tasks: TaskTracker,
    pub request_timeout: Duration,
    pub body_limit: usize,
}

impl AppState {
    /// Runs a write in its own tracked task, so that a timeout or a lost
    /// client never cancels it part-way.
    pub async fn write<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        let id = REQUEST_ID.try_with(Clone::clone).unwrap_or_default();
        let task = self.tasks.spawn(REQUEST_ID.scope(id, work));
        task.await.map_err(ApiError::internal)?
    }
}

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

async fn request_context(
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

/// The service's router.
pub fn router(state: AppState) -> Router {
    let limit = state.body_limit;
    routes::routes()
        .fallback(|| async { ApiError::not_found("route") })
        .layer(DefaultBodyLimit::max(limit))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_context,
        ))
        .with_state(state)
}

/// Serves until `shutdown` completes, then stops taking new connections,
/// finishes the requests under way, and waits for every write task.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let tasks = state.tasks.clone();
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await?;
    tasks.close();
    tasks.wait().await;
    Ok(())
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

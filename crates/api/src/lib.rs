//! Ledger REST service (ADR-0009, spec §9, step 2.07).
//!
//! The contract is `openapi/mezgeb.v1.yaml`; this crate implements it. A
//! request goes through:
//!
//! 1. **request ID**: the client's `X-Request-Id` if it is a UUID,
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
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::{self};
use mezgeb_core::Chart;
use mezgeb_engine::PostOptions;
use mezgeb_ops::TemplateRegistry;
use sqlx::PgPool;
use tokio_util::task::TaskTracker;

use crate::auth::Permissions;
use crate::http::middleware::request_context;
use crate::problem::{ApiError, REQUEST_ID};

/// Everything a handler needs.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub chart: Arc<Chart>,
    /// The permission matrix: what each role grants.
    pub permissions: Arc<Permissions>,
    /// How long an access token lives.
    pub token_ttl: Duration,
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

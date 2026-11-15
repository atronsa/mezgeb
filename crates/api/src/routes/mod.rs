//! The handlers, one file per tag of `openapi/mezgeb.v1.yaml`. Each one
//! authenticates the caller and checks the operation's scope before it reads
//! the body or touches the ledger, then calls the templates and the engine.
//!
//! A handler is a thin adapter: it parses the request, calls `mezgeb-ops`
//! or `mezgeb-engine`, and answers with a `wire` type. No SQL and no ledger
//! rules live in this crate.

use axum::Router;

use crate::AppState;

mod accounts;
mod common;
mod corrections;
mod entries;
mod holds;
mod oauth;
mod offline;
mod operations;
mod periods;
mod reports;
mod templates;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(accounts::router())
        .merge(templates::router())
        .merge(operations::router())
        .merge(entries::router())
        .merge(corrections::router())
        .merge(holds::router())
        .merge(periods::router())
        .merge(reports::router())
        .merge(offline::router())
        .merge(oauth::router())
}

//! What every request goes through: a request ID, the caller's address,
//! the timeout, one access log line (see the crate documentation), and, for
//! every change, refusal and read across clients, an audit event
//! (ADR-0018). Refusals of callers who have not authenticated are audited in
//! summary ([`super::refusals`], ADR-0021).

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use mezgeb_core::ClientId;

use crate::AppState;
use crate::auth::rate::AddressSource;
use crate::problem::{ApiError, REQUEST_ID};

/// The caller's address, for the limit per address (ADR-0021); `None` when
/// it is not known.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClientAddress(pub Option<IpAddr>);

/// The caller's address: the TCP peer, or the last `X-Forwarded-For` entry
/// when the operator says its proxy adds one.
fn client_address(request: &Request, source: AddressSource) -> Option<IpAddr> {
    let peer = || {
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
    };
    match source {
        AddressSource::Peer => peer(),
        AddressSource::ForwardedFor => forwarded_for(request.headers()).or_else(peer),
    }
}

/// The last address in the last `X-Forwarded-For` header: the one the
/// nearest proxy added. Earlier entries are whatever the caller sent.
fn forwarded_for(headers: &axum::http::HeaderMap) -> Option<IpAddr> {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .next_back()?
        .to_str()
        .ok()?
        .rsplit(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Who made a request, filled in by authentication, for the access log
/// and the audit log.
#[derive(Clone, Default)]
pub struct LoggedClient(pub Arc<Mutex<Option<LoggedCaller>>>);

/// The caller, as the logs record it.
#[derive(Clone, Debug)]
pub struct LoggedCaller {
    pub client: ClientId,
    pub actor: String,
    /// Reads every client's data (`accounts:read_all`): its reads are
    /// audited too (TM-16).
    pub sees_all: bool,
}

/// The error code of a refused request, set by the error responses, for
/// the audit log.
#[derive(Clone, Copy, Debug)]
pub struct ProblemCode(pub &'static str);

/// Further facts a handler adds to its request's audit event, such as the
/// before and after of a change of configuration. A JSON object.
#[derive(Clone, Debug)]
pub struct AuditDetails(pub serde_json::Map<String, serde_json::Value>);

/// Who the request named, when there was no caller to authenticate (the
/// token endpoint: the credential it tried).
#[derive(Clone, Debug)]
pub struct AuditActor(pub String);

fn outcome(status: u16) -> &'static str {
    match status {
        200..=299 => "ok",
        401 => "unauthenticated",
        403 => "forbidden",
        404 => "not_found",
        429 => "rate_limited",
        400..=499 => "refused",
        _ => "failed",
    }
}

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
    let address = client_address(&request, state.limits.address.source);
    request.extensions_mut().insert(ClientAddress(address));
    let method = request.method().clone();
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned());
    let route = matched.clone().unwrap_or_else(|| "(no route)".to_owned());
    let path: String = request.uri().path().chars().take(256).collect();
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
    let caller = logged.0.lock().ok().and_then(|c| c.clone());
    let status = response.status().as_u16();
    tracing::info!(
        target: "access",
        method = %method,
        route = %route,
        status,
        latency_ms = started.elapsed().as_millis() as u64,
        request_id = %id,
        client = caller.as_ref().map(|c| c.client.to_string()).unwrap_or_default(),
    );

    // The audit log: every change, every refusal of authentication or
    // scope, every read across clients.
    let reads = matches!(method, axum::http::Method::GET | axum::http::Method::HEAD);
    let across = caller.as_ref().is_some_and(|c| c.sees_all);
    // A refusal of a caller who has not authenticated is written only if
    // it is the first of its kind in the minute; the rest are counted.
    let unauthenticated_refusal = caller.is_none() && (400..500).contains(&status);
    let tried = response
        .extensions()
        .get::<AuditActor>()
        .map(|a| a.0.clone());
    let summarised = unauthenticated_refusal
        && !state
            .refusals
            .admit(tried.as_deref().unwrap_or("anonymous"), outcome(status));
    if (!reads || status == 401 || status == 403 || across) && !summarised {
        let mut details = serde_json::Map::new();
        details.insert("status".into(), status.into());
        if let Some(ProblemCode(code)) = response.extensions().get::<ProblemCode>() {
            details.insert("code".into(), (*code).into());
        }
        if let Some(AuditDetails(more)) = response.extensions().get::<AuditDetails>() {
            details.extend(more.clone());
        }
        let actor = caller.as_ref().map(|c| c.actor.clone()).or(tried);
        let event = mezgeb_engine::NewAuditEvent {
            client: caller.as_ref().map(|c| c.client),
            actor,
            action: format!("{method} {}", matched.as_deref().unwrap_or("/unmatched")),
            target: Some(path),
            request_id: Some(id),
            outcome: outcome(status).to_owned(),
            details: serde_json::Value::Object(details).to_string(),
        };
        let pool = state.pool.clone();
        // Tracked, so a graceful shutdown waits for it.
        state.tasks.spawn(async move {
            let written = match pool.acquire().await {
                Ok(mut conn) => mezgeb_engine::append_audit(&mut conn, &event, crate::wire::now())
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(cause) = written {
                tracing::error!(%cause, action = %event.action, "an audit event was not recorded");
            }
        });
    }
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

    #[test]
    fn only_the_nearest_forwarded_address_is_trusted() {
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(forwarded_for(&headers), None);
        // The caller wrote the first entry; the proxy added the last.
        headers.append(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.9, 198.51.100.4"),
        );
        assert_eq!(forwarded_for(&headers), "198.51.100.4".parse().ok());
        headers.append("x-forwarded-for", HeaderValue::from_static("2001:db8::1"));
        assert_eq!(forwarded_for(&headers), "2001:db8::1".parse().ok());
        headers.append("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert_eq!(forwarded_for(&headers), None);
    }
}

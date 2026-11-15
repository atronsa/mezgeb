//! OAuth: the token endpoint (RFC 6749 §4.4, client credentials). The only
//! operation without a bearer token: the caller authenticates with its
//! credential's ID and secret instead ([`crate::auth::oauth`]).

use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Form, Router};

use crate::AppState;
use crate::auth::oauth::{OAuthError, TokenRequest, basic_credentials, issue};

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/oauth/token", post(token))
}

async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    form: Result<Form<TokenRequest>, FormRejection>,
) -> Response {
    let Ok(Form(request)) = form else {
        return OAuthError::invalid_request(
            "The body must be application/x-www-form-urlencoded with grant_type.",
        )
        .into_response();
    };
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    // The audit log names the credential tried, never its secret.
    let tried = basic_credentials(authorization).map(|(id, _)| format!("credential:{id}"));
    let mut response = match issue(&state, authorization, request).await {
        Ok(token) => token.into_response(),
        Err(error) => error.into_response(),
    };
    if let Some(tried) = tried {
        response
            .extensions_mut()
            .insert(crate::http::middleware::AuditActor(tried));
    }
    response
}

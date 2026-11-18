//! The OAuth 2.0 token endpoint, `POST /oauth/token` (RFC 6749 §4.4,
//! client credentials; ADR-0017).
//!
//! The client authenticates with HTTP Basic: the credential's ID as the
//! user name and its secret as the password (RFC 6749 §2.3.1). The secret is
//! checked against its Argon2id hash off the async runtime. An unknown ID is
//! checked against a dummy hash, so it takes as long to refuse as a wrong
//! secret and does not reveal which IDs exist.
//!
//! Failed logins are limited per credential, in the whole service and, if
//! the operator turns it on, per address (ADR-0021): over any of them the
//! endpoint answers `429 slow_down` before looking anything up or hashing.
//!
//! The answer is a random token that lives `MEZGEB_TOKEN_TTL_SECONDS`
//! (15 minutes by default), with the scopes the credential's roles grant,
//! or the subset asked for with `scope`. Errors follow RFC 6749 §5.2
//! (`{"error": "invalid_client"}`), not Problem Details: OAuth clients
//! expect them.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::OnceLock;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use mezgeb_core::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::wire::now;

/// An RFC 6749 §5.2 error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: String,
}

impl OAuthError {
    fn new(status: StatusCode, error: &'static str, description: impl Into<String>) -> OAuthError {
        OAuthError {
            status,
            error,
            description: description.into(),
        }
    }

    /// The credential is unknown, revoked, or its secret is wrong: all
    /// answered the same way.
    pub fn invalid_client() -> OAuthError {
        OAuthError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "Client authentication failed.",
        )
    }

    pub fn invalid_request(description: impl Into<String>) -> OAuthError {
        OAuthError::new(StatusCode::BAD_REQUEST, "invalid_request", description)
    }

    pub fn error(&self) -> &'static str {
        self.error
    }

    fn slow_down(description: &str) -> OAuthError {
        OAuthError::new(StatusCode::TOO_MANY_REQUESTS, "slow_down", description)
    }

    fn server(description: impl std::fmt::Display) -> OAuthError {
        tracing::error!(cause = %description, "the token endpoint failed");
        OAuthError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "The token could not be issued; retry.",
        )
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    error_description: &'a str,
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.error,
            error_description: &self.description,
        };
        let mut response = (self.status, axum::Json(body)).into_response();
        response
            .extensions_mut()
            .insert(crate::http::middleware::ProblemCode(self.error));
        let headers = response.headers_mut();
        no_store(headers);
        if self.status == StatusCode::UNAUTHORIZED {
            headers.insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"mezgeb\""),
            );
        }
        if matches!(
            self.status,
            StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS
        ) {
            headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

fn no_store(headers: &mut axum::http::HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
}

/// The form fields of a token request.
#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub grant_type: Option<String>,
    pub scope: Option<String>,
    /// Credentials in the body (`client_secret_post`) are refused: only
    /// HTTP Basic is accepted, so secrets never sit in a logged body.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

/// A successful answer (RFC 6749 §5.1).
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub scope: String,
}

impl IntoResponse for TokenResponse {
    fn into_response(self) -> Response {
        let mut response = (StatusCode::OK, axum::Json(self)).into_response();
        no_store(response.headers_mut());
        response
    }
}

/// The credential ID and secret of an `Authorization: Basic` header.
pub fn basic_credentials(header: Option<&str>) -> Option<(Uuid, String)> {
    let encoded = header?.strip_prefix("Basic ")?.trim();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (id, secret) = text.split_once(':')?;
    if secret.is_empty() {
        return None;
    }
    Some((id.parse().ok()?, secret.to_owned()))
}

/// The scopes to grant: all the roles allow, or the subset asked for.
pub fn granted_scopes(
    allowed: &BTreeSet<String>,
    asked: Option<&str>,
) -> Result<BTreeSet<String>, OAuthError> {
    let Some(asked) = asked.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(allowed.clone());
    };
    let asked: BTreeSet<String> = asked
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(extra) = asked.iter().find(|s| !allowed.contains(*s)) {
        return Err(OAuthError::new(
            StatusCode::BAD_REQUEST,
            "invalid_scope",
            format!("The credential's roles do not grant {extra:?}."),
        ));
    }
    Ok(asked)
}

/// A hash of no real secret, checked when the ID is unknown so the answer
/// takes as long as for a wrong secret.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        mezgeb_engine::hash_secret("mzs_no-credential-has-this-secret").unwrap_or_default()
    })
}

/// Handles a token request from `address`.
pub async fn issue(
    state: &AppState,
    authorization: Option<&str>,
    address: Option<IpAddr>,
    request: TokenRequest,
) -> Result<TokenResponse, OAuthError> {
    if request.client_id.is_some() || request.client_secret.is_some() {
        return Err(OAuthError::invalid_request(
            "Send the client ID and secret with HTTP Basic authentication, not in the body.",
        ));
    }
    let (id, secret) = basic_credentials(authorization).ok_or_else(OAuthError::invalid_client)?;
    // Before any hashing: each attempt costs an Argon2 hash, and a secret
    // must not be guessable at speed. Unknown IDs are limited too.
    if !state
        .limits
        .logins
        .take(id, super::rate::TOKEN_RATE, super::rate::TOKEN_BURST)
    {
        return Err(OAuthError::slow_down(
            "Too many token requests for this credential; retry after Retry-After seconds.",
        ));
    }
    // A caller inventing a new ID each time passes the limit above; the
    // failed logins of the whole service, and of its address, bound it.
    // One is reserved now and given back if the secret is right.
    if !state.limits.reserve_login(address) {
        return Err(OAuthError::slow_down(
            "Too many failed token requests; retry after Retry-After seconds.",
        ));
    }
    match request.grant_type.as_deref() {
        Some("client_credentials") => {}
        Some(_) => {
            return Err(OAuthError::new(
                StatusCode::BAD_REQUEST,
                "unsupported_grant_type",
                "Only grant_type=client_credentials is supported.",
            ));
        }
        None => return Err(OAuthError::invalid_request("grant_type is required.")),
    }
    let mut conn = state.pool.acquire().await.map_err(OAuthError::server)?;
    let found = mezgeb_engine::credential_for_login(&mut conn, id)
        .await
        .map_err(OAuthError::server)?;
    drop(conn);
    let hash = found
        .as_ref()
        .map_or_else(|| dummy_hash().to_owned(), |(_, _, hash)| hash.clone());
    let matches = tokio::task::spawn_blocking(move || mezgeb_engine::verify_secret(&hash, &secret))
        .await
        .map_err(OAuthError::server)?;
    let Some((credential, _client, _)) = found.filter(|_| matches) else {
        tracing::warn!(credential = %id, "token refused: unknown credential or wrong secret");
        return Err(OAuthError::invalid_client());
    };
    state.limits.release_login(address);
    let allowed = state.permissions.scopes(&credential.roles);
    let scopes = granted_scopes(&allowed, request.scope.as_deref())?;
    let scopes: Vec<String> = scopes.into_iter().collect();

    let (token, digest) = mezgeb_engine::generate_token().map_err(OAuthError::server)?;
    let issued = now();
    let ttl = state.token_ttl.as_secs();
    let lifetime = i64::try_from(state.token_ttl.as_micros()).unwrap_or(i64::MAX);
    let expires = Timestamp::from_micros(issued.as_micros().saturating_add(lifetime));
    let mut conn = state.pool.acquire().await.map_err(OAuthError::server)?;
    mezgeb_engine::store_token(&mut conn, &digest, credential.id, &scopes, expires)
        .await
        .map_err(OAuthError::server)?;
    // Tokens that expired an hour ago are of no use to anyone.
    let stale = Timestamp::from_micros(issued.as_micros().saturating_sub(3_600_000_000));
    if let Err(e) = mezgeb_engine::purge_expired_tokens(&mut conn, stale).await {
        tracing::warn!(cause = %e, "could not purge expired tokens");
    }
    tracing::info!(credential = %credential.id, actor = %credential.actor, "token issued");
    Ok(TokenResponse {
        access_token: token,
        token_type: "Bearer",
        expires_in: ttl,
        scope: scopes.join(" "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(text: &str) -> String {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(text)
        )
    }

    #[test]
    fn basic_credentials_are_parsed() {
        let id = "0192f3c4-0000-7000-8000-0000000c0001";
        assert_eq!(
            basic_credentials(Some(&basic(&format!("{id}:s3cret")))),
            id.parse().ok().map(|id| (id, "s3cret".to_owned()))
        );
        for bad in [
            None,
            Some("Bearer x".to_owned()),
            Some("Basic !!!".to_owned()),
            Some(basic("no-colon")),
            Some(basic(&format!("{id}:"))),
            Some(basic("not-a-uuid:secret")),
        ] {
            assert_eq!(basic_credentials(bad.as_deref()), None, "{bad:?}");
        }
    }

    #[test]
    fn scopes_can_only_narrow() -> Result<(), OAuthError> {
        let allowed: BTreeSet<String> = ["accounts:read", "operations:write"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(granted_scopes(&allowed, None)?, allowed);
        assert_eq!(granted_scopes(&allowed, Some("  "))?, allowed);
        assert_eq!(
            granted_scopes(&allowed, Some("accounts:read"))?
                .into_iter()
                .collect::<Vec<_>>(),
            ["accounts:read"]
        );
        assert_eq!(
            granted_scopes(&allowed, Some("accounts:read periods:close")).map_err(|e| e.error),
            Err("invalid_scope")
        );
        Ok(())
    }
}

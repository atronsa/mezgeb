//! Extractors: who is calling, the body, the idempotency key, path IDs.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use mezgeb_core::IdempotencyKey;
use serde::de::DeserializeOwned;

use crate::AppState;
use crate::auth::Principal;
use crate::http::middleware::LoggedClient;
use crate::problem::ApiError;

/// The authenticated caller.
pub struct Caller(pub Principal);

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Caller, ApiError> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let address = parts
            .extensions
            .get::<crate::http::middleware::ClientAddress>()
            .and_then(|a| a.0);
        let principal = crate::auth::authenticate(state, header, address).await?;
        if let Some(logged) = parts.extensions.get::<LoggedClient>()
            && let Ok(mut slot) = logged.0.lock()
        {
            *slot = Some(crate::http::middleware::LoggedCaller {
                client: principal.client,
                actor: principal.actor.as_str().to_owned(),
                sees_all: principal.sees_all(),
            });
        }
        // After the caller is known, so a client over its own limit is
        // audited as itself.
        crate::auth::admit(state, &principal)?;
        Ok(Caller(principal))
    }
}

/// Query parameters. A missing, malformed or unknown parameter is
/// `400 invalid_request`, as Problem Details (axum's own `Query` answers in
/// plain text).
pub struct QueryParams<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for QueryParams<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<QueryParams<T>, ApiError> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(value)| QueryParams(value))
            .map_err(|rejection| ApiError::invalid("/query", rejection.body_text()))
    }
}

/// A JSON request body. Every way the body can be wrong, including too
/// large, is `400 invalid_request`.
pub struct JsonBody<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for JsonBody<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<JsonBody<T>, ApiError> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(match rejection {
                JsonRejection::JsonDataError(e) => ApiError::invalid("", e.body_text()),
                JsonRejection::JsonSyntaxError(e) => ApiError::invalid("", e.body_text()),
                JsonRejection::MissingJsonContentType(_) => ApiError::invalid(
                    "",
                    "The body must be JSON, with Content-Type: application/json.",
                ),
                JsonRejection::BytesRejection(_) => {
                    ApiError::invalid("", "The body is too large or could not be read.")
                }
                other => ApiError::invalid("", other.body_text()),
            }),
        }
    }
}

/// The `Idempotency-Key` header of a write.
pub struct IdemKey(pub IdempotencyKey);

impl<S: Send + Sync> FromRequestParts<S> for IdemKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<IdemKey, ApiError> {
        let value = parts.headers.get("idempotency-key").ok_or_else(|| {
            ApiError::new(
                "idempotency_key_required",
                "This write needs an Idempotency-Key header.",
            )
        })?;
        let text = value.to_str().map_err(|_| {
            ApiError::invalid(
                "/headers/Idempotency-Key",
                "The Idempotency-Key is not text.",
            )
        })?;
        IdempotencyKey::new(text)
            .map(IdemKey)
            .map_err(|e| ApiError::invalid("/headers/Idempotency-Key", e.to_string()))
    }
}

/// Parses a path or query ID.
pub fn parse_id<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, ApiError> {
    text.parse().map_err(|_| ApiError::not_found(what))
}

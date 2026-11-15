//! Extractors: who is calling.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::auth::Principal;
use crate::problem::ApiError;
use crate::{AppState, LoggedClient};

/// The authenticated caller.
pub struct Caller(pub Principal);

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Caller, ApiError> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let principal = state.tokens.authenticate(header)?;
        if let Some(logged) = parts.extensions.get::<LoggedClient>()
            && let Ok(mut slot) = logged.0.lock()
        {
            *slot = Some(principal.client);
        }
        Ok(Caller(principal))
    }
}

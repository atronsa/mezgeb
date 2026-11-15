//! RFC 9457 Problem Details, with the stable codes of `docs/api/errors.md`.
//!
//! Every error the service returns is built here. A storage error's message
//! is logged, never sent: the client gets `internal_error` and the request
//! ID to quote.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

mod catalogue;
mod mapping;

pub use catalogue::CATALOGUE;

tokio::task_local! {
    /// The current request's ID, for the `instance` of its errors.
    pub static REQUEST_ID: String;
}

/// One field at fault in an `invalid_request`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldError {
    pub pointer: String,
    pub detail: String,
}

/// The body of an error, as the client will see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    kind: String,
    title: String,
    status: u16,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance: Option<String>,
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<FieldError>,
}

/// An error the service returns. Boxed, so that every `Result` that can
/// carry one stays small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError(Box<Problem>);

impl ApiError {
    /// An error with one of the catalogue's codes.
    pub fn new(code: &'static str, detail: impl Into<String>) -> ApiError {
        let (status, title) = CATALOGUE
            .iter()
            .find(|(c, ..)| *c == code)
            .map_or((500, "Internal error"), |(_, s, t)| (*s, *t));
        ApiError(Box::new(Problem {
            kind: format!("urn:mezgeb:problem:{code}"),
            title: title.to_owned(),
            status,
            detail: detail.into(),
            instance: None,
            code,
            rule: None,
            line: None,
            role: None,
            name: None,
            errors: Vec::new(),
        }))
    }

    pub fn code(&self) -> &'static str {
        self.0.code
    }

    pub fn status(&self) -> u16 {
        self.0.status
    }

    /// The body, for tests.
    pub fn problem(&self) -> &Problem {
        &self.0
    }

    fn with_rule(mut self, rule: u16) -> ApiError {
        self.0.rule = Some(rule);
        self
    }

    fn with_line(mut self, line: Option<usize>) -> ApiError {
        self.0.line = line;
        self
    }

    pub fn with_role(mut self, role: impl Into<String>) -> ApiError {
        self.0.role = Some(role.into());
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> ApiError {
        self.0.name = Some(name.into());
        self
    }

    pub fn invalid(pointer: &str, detail: impl Into<String>) -> ApiError {
        let detail = detail.into();
        let mut error = ApiError::new("invalid_request", detail.clone());
        error.0.errors.push(FieldError {
            pointer: pointer.to_owned(),
            detail,
        });
        error
    }

    pub fn unauthenticated() -> ApiError {
        ApiError::new("unauthenticated", "A valid bearer token is required.")
    }

    pub fn forbidden(scope: &str) -> ApiError {
        ApiError::new("forbidden", format!("This needs the scope {scope}."))
    }

    pub fn not_found(what: &str) -> ApiError {
        ApiError::new("not_found", format!("No such {what}."))
    }

    pub fn internal(cause: impl std::fmt::Display) -> ApiError {
        tracing::error!(%cause, "internal error");
        ApiError::new(
            "internal_error",
            "Something failed inside the ledger. The request may or may not have taken effect: \
             retry with the same Idempotency-Key, or look the entry up by its key.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(mut self) -> Response {
        if self.0.instance.is_none() {
            self.0.instance = REQUEST_ID.try_with(|id| format!("urn:uuid:{id}")).ok();
        }
        let status =
            StatusCode::from_u16(self.0.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_vec(&*self.0).unwrap_or_default();
        let mut response = (status, body).into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if matches!(self.0.status, 429 | 503) {
            headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use mezgeb_core::PostingError;

    use super::*;

    #[test]
    fn problems_serialize_as_rfc_9457() -> Result<(), serde_json::Error> {
        let response = ApiError::from(PostingError::InsufficientFunds { line: 0 }).into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(&b"application/problem+json"[..])
        );
        let e = ApiError::from(PostingError::InsufficientFunds { line: 0 });
        let json: serde_json::Value = serde_json::to_value(&*e.0)?;
        assert_eq!(json["type"], "urn:mezgeb:problem:insufficient_funds");
        assert_eq!(json["status"], 422);
        assert_eq!(json["rule"], 1012);
        Ok(())
    }
}

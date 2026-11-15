//! RFC 9457 Problem Details, with the stable codes of `docs/api/errors.md`.
//!
//! Every error the service returns is built here. A storage error's message
//! is logged, never sent: the client gets `internal_error` and the request
//! ID to quote.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use mezgeb_core::{LedgerError, PostingError};
use mezgeb_ops::TemplateError;
use serde::Serialize;

tokio::task_local! {
    /// The current request's ID, for the `instance` of its errors.
    pub static REQUEST_ID: String;
}

/// Every code: HTTP status and title. The same list as the contract's
/// `ErrorCode` and `docs/api/errors.md`; a test keeps them equal.
pub const CATALOGUE: &[(&str, u16, &str)] = &[
    ("invalid_request", 400, "The request is malformed"),
    (
        "idempotency_key_required",
        400,
        "An Idempotency-Key header is required",
    ),
    ("unauthenticated", 401, "Authentication is required"),
    ("forbidden", 403, "The caller may not do this"),
    ("not_found", 404, "Not found"),
    (
        "idempotency_key_reused",
        409,
        "The idempotency key was used for a different request",
    ),
    ("rate_limited", 429, "Too many requests"),
    ("internal_error", 500, "Internal error"),
    ("not_implemented", 501, "Not implemented yet"),
    ("busy", 503, "The accounts are busy; retry later"),
    (
        "contention",
        503,
        "Conflicted with concurrent operations; retry later",
    ),
    ("no_open_period", 503, "No accounting period is open"),
    ("too_few_lines", 422, "An entry needs at least 2 lines"),
    ("too_many_lines", 422, "An entry has too many lines"),
    ("metadata_too_large", 422, "The metadata is too large"),
    ("amount_not_positive", 422, "A line amount is not positive"),
    (
        "account_not_found",
        422,
        "A line names an account that does not exist",
    ),
    ("account_not_open", 422, "An account is not open"),
    (
        "currency_mismatch",
        422,
        "A line's currency is not its account's",
    ),
    ("duplicate_line", 422, "Two lines share an account and side"),
    ("amount_overflow", 422, "The amounts are too large"),
    ("unbalanced", 422, "Debits and credits differ"),
    (
        "account_total_overflow",
        422,
        "An account's totals would be too large",
    ),
    ("insufficient_funds", 422, "An account would go below zero"),
    ("invalid_template_id", 404, "The template ID is malformed"),
    ("unknown_template", 404, "No such template"),
    ("unknown_template_version", 404, "No such template version"),
    (
        "missing_amount",
        422,
        "An amount the template needs is missing",
    ),
    (
        "unexpected_amount",
        422,
        "The template does not use this amount",
    ),
    ("non_positive_amount", 422, "An amount is not positive"),
    (
        "missing_account",
        422,
        "A role the template books has no account",
    ),
    ("unexpected_account", 422, "The template has no such role"),
    (
        "bound_account_not_found",
        422,
        "The account bound to a role does not exist",
    ),
    (
        "account_not_allowed",
        422,
        "The account is not allowed for its role",
    ),
    (
        "same_account",
        422,
        "Two roles are bound to the same account",
    ),
    ("reserved_metadata", 422, "The metadata sets a reserved key"),
    (
        "template_defect",
        500,
        "The template books a role it does not declare",
    ),
    (
        "amount_not_whole_balance",
        422,
        "The amount is not the item's whole balance",
    ),
    (
        "account_not_new",
        422,
        "The item account has already been used",
    ),
    (
        "fee_exceeds_amount",
        422,
        "The fees are more than the amount",
    ),
    (
        "amount_currency_mismatch",
        422,
        "The amounts are in different currencies",
    ),
    ("amount_too_large", 422, "An amount is too large"),
    (
        "missing_metadata",
        422,
        "A metadata key the template requires is missing",
    ),
    ("invalid_metadata", 422, "A metadata value is not allowed"),
    (
        "missing_refunded_entry",
        422,
        "A refund must name the entry it refunds",
    ),
    (
        "unexpected_refunded_entry",
        422,
        "Only a refund template names an entry to refund",
    ),
    (
        "invalid_reference",
        422,
        "The chart reference is invalid or not open for accounts",
    ),
    (
        "duplicate_reference",
        409,
        "An account already exists for this reference",
    ),
    (
        "invalid_status_change",
        409,
        "The account cannot change to that status",
    ),
    ("already_reversed", 409, "The entry is already reversed"),
    (
        "cannot_reverse_reversal",
        409,
        "A reversal cannot be reversed",
    ),
    (
        "reason_not_allowed",
        422,
        "The reason is unknown or not for this kind of entry",
    ),
    (
        "maker_is_checker",
        403,
        "The approver must be a different person",
    ),
    ("hold_not_active", 409, "The hold has already ended"),
    ("hold_expired", 409, "The hold has expired"),
    (
        "capture_exceeds_hold",
        422,
        "The capture is larger than the hold",
    ),
    (
        "invalid_held_account",
        422,
        "The held account must be a different, unused account of the same type",
    ),
    (
        "account_is_held",
        422,
        "The account is a hold's held account",
    ),
    (
        "not_refundable",
        409,
        "Only a standard entry can be refunded",
    ),
    (
        "refund_of_reversed_entry",
        409,
        "The entry has been reversed",
    ),
    (
        "refund_line_not_allowed",
        422,
        "The refund moves an account the original did not move that way",
    ),
    (
        "refund_exceeds_original",
        422,
        "The refunds would give back more than the original moved",
    ),
    (
        "refunded_entry_cannot_be_reversed",
        409,
        "The entry has refunds",
    ),
    ("cannot_reverse_refund", 409, "A refund cannot be reversed"),
    ("period_not_closed", 409, "The period is not closed"),
    ("period_label_taken", 409, "A period with this label exists"),
    (
        "period_close_findings",
        409,
        "The period did not close: verification found problems",
    ),
    (
        "adjustment_not_pending",
        409,
        "The adjustment request is no longer pending",
    ),
];

/// The posting rule codes, by rule number (1001–1012).
const POSTING: [&str; 12] = [
    "too_few_lines",
    "too_many_lines",
    "metadata_too_large",
    "amount_not_positive",
    "account_not_found",
    "account_not_open",
    "currency_mismatch",
    "duplicate_line",
    "amount_overflow",
    "unbalanced",
    "account_total_overflow",
    "insufficient_funds",
];

/// The template rule codes, by rule number (2001–2022).
const TEMPLATE: [&str; 22] = [
    "invalid_template_id",
    "unknown_template",
    "unknown_template_version",
    "missing_amount",
    "unexpected_amount",
    "non_positive_amount",
    "missing_account",
    "unexpected_account",
    "bound_account_not_found",
    "account_not_allowed",
    "same_account",
    "reserved_metadata",
    "template_defect",
    "amount_not_whole_balance",
    "account_not_new",
    "fee_exceeds_amount",
    "amount_currency_mismatch",
    "amount_too_large",
    "missing_metadata",
    "invalid_metadata",
    "missing_refunded_entry",
    "unexpected_refunded_entry",
];

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

impl From<PostingError> for ApiError {
    fn from(error: PostingError) -> ApiError {
        let rule = error.code();
        let code = POSTING
            .get(usize::from(rule.saturating_sub(1001)))
            .copied()
            .unwrap_or("internal_error");
        let line = match error {
            PostingError::AmountNotPositive { line }
            | PostingError::AccountNotFound { line }
            | PostingError::AccountNotOpen { line, .. }
            | PostingError::CurrencyMismatch { line, .. }
            | PostingError::DuplicateLine { line }
            | PostingError::AccountTotalOverflow { line }
            | PostingError::InsufficientFunds { line } => Some(line),
            _ => None,
        };
        ApiError::new(code, error.to_string())
            .with_rule(rule)
            .with_line(line)
    }
}

impl From<TemplateError> for ApiError {
    fn from(error: TemplateError) -> ApiError {
        let rule = error.code();
        let code = TEMPLATE
            .get(usize::from(rule.saturating_sub(2001)))
            .copied()
            .unwrap_or("internal_error");
        let detail = error.to_string();
        let mut api = ApiError::new(code, detail).with_rule(rule);
        match error {
            TemplateError::MissingAmount(name)
            | TemplateError::UnexpectedAmount(name)
            | TemplateError::AmountNotPositive(name)
            | TemplateError::CurrencyMismatch(name)
            | TemplateError::MissingMetadata(name)
            | TemplateError::InvalidMetadata(name) => api = api.with_name(name),
            TemplateError::MissingAccount(role)
            | TemplateError::UnexpectedAccount(role)
            | TemplateError::AmountNotBalance(role)
            | TemplateError::AccountNotNew(role)
            | TemplateError::AccountNotFound { role, .. }
            | TemplateError::AccountNotAllowed { role, .. }
            | TemplateError::SameAccount { role, .. } => api = api.with_role(role),
            TemplateError::UndeclaredRole(role) => {
                tracing::error!(%role, "a template books an undeclared role");
            }
            _ => {}
        }
        api
    }
}

impl From<LedgerError> for ApiError {
    fn from(error: LedgerError) -> ApiError {
        let detail = error.to_string();
        let code = match &error {
            LedgerError::Posting(e) => return ApiError::from(*e),
            LedgerError::Chart(_) => "invalid_reference",
            LedgerError::DuplicateReference(_) => "duplicate_reference",
            LedgerError::AccountNotFound(_)
            | LedgerError::EntryNotFound(_)
            | LedgerError::HoldNotFound(_)
            | LedgerError::AdjustmentNotFound => "not_found",
            LedgerError::Status(_) => "invalid_status_change",
            LedgerError::AlreadyReversed(_) => "already_reversed",
            LedgerError::CannotReverseReversal(_) => "cannot_reverse_reversal",
            LedgerError::Contention(_) => "contention",
            LedgerError::Busy => "busy",
            LedgerError::IdempotencyKeyReused => "idempotency_key_reused",
            LedgerError::ReasonNotAllowed(_) => "reason_not_allowed",
            LedgerError::MakerIsChecker => "maker_is_checker",
            LedgerError::HoldNotActive { .. } => "hold_not_active",
            LedgerError::HoldExpired(_) => "hold_expired",
            LedgerError::CaptureExceedsHold(_) => "capture_exceeds_hold",
            LedgerError::InvalidHeldAccount(_) => "invalid_held_account",
            LedgerError::AccountIsHeld(_) => "account_is_held",
            LedgerError::NotRefundable(_) => "not_refundable",
            LedgerError::RefundOfReversedEntry(_) => "refund_of_reversed_entry",
            LedgerError::RefundLineNotAllowed { line } => {
                return ApiError::new("refund_line_not_allowed", detail).with_line(Some(*line));
            }
            LedgerError::RefundExceedsOriginal { line } => {
                return ApiError::new("refund_exceeds_original", detail).with_line(Some(*line));
            }
            LedgerError::RefundedEntryCannotBeReversed(_) => "refunded_entry_cannot_be_reversed",
            LedgerError::CannotReverseRefund(_) => "cannot_reverse_refund",
            LedgerError::NoOpenPeriod => "no_open_period",
            LedgerError::PeriodNotClosed(_) => "period_not_closed",
            LedgerError::AdjustmentNotPending => "adjustment_not_pending",
            // The service never builds these: a defect, or the storage.
            LedgerError::Storage(_)
            | LedgerError::DuplicateAccount(_)
            | LedgerError::DuplicateEntry(_)
            | LedgerError::DuplicateHold(_)
            | LedgerError::AuthorizationRequired
            | LedgerError::AuthorizationNotAllowed
            | LedgerError::HoldNotExpired(_)
            | LedgerError::RefundAsAdjustment => return ApiError::internal(detail),
        };
        ApiError::new(code, detail)
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
    use super::*;
    use mezgeb_core::Currency;

    #[test]
    fn rules_map_to_their_codes() {
        let e = ApiError::from(PostingError::InsufficientFunds { line: 2 });
        assert_eq!(
            (e.code(), e.status(), e.0.rule, e.0.line),
            ("insufficient_funds", 422, Some(1012), Some(2))
        );
        let e = ApiError::from(PostingError::Unbalanced {
            currency: Currency::ETB,
            debits: 1,
            credits: 2,
        });
        assert_eq!((e.code(), e.0.rule), ("unbalanced", Some(1010)));
        let e = ApiError::from(TemplateError::AccountNotNew("pending_withdrawal".into()));
        assert_eq!(
            (e.code(), e.0.rule, e.0.role.as_deref()),
            ("account_not_new", Some(2015), Some("pending_withdrawal"))
        );
        let e = ApiError::from(TemplateError::UnknownTemplate("x.v1".into()));
        assert_eq!((e.code(), e.status()), ("unknown_template", 404));
        let e = ApiError::from(LedgerError::Storage("password=secret".into()));
        assert_eq!(e.code(), "internal_error");
        assert!(
            !e.0.detail.contains("secret"),
            "storage errors are never sent"
        );
    }

    #[test]
    fn every_code_used_is_in_the_catalogue() {
        for code in POSTING.iter().chain(TEMPLATE.iter()) {
            assert!(CATALOGUE.iter().any(|(c, ..)| c == code), "{code}");
        }
    }

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

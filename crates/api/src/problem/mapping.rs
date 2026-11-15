//! From the ledger's errors to the service's codes.

use mezgeb_core::{LedgerError, PostingError};
use mezgeb_ops::TemplateError;

use super::ApiError;
use super::catalogue::{POSTING, TEMPLATE};

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
}

//! The codes: every one, and the posting and template rules by number.

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
    (
        "offline_unavailable",
        503,
        "Offline payments are not configured",
    ),
    (
        "attestation_unsupported",
        422,
        "This kind of attestation is not verified yet",
    ),
    (
        "public_key_registered",
        409,
        "The public key is registered with other details",
    ),
    (
        "owner_not_a_wallet",
        422,
        "The owner account is not an open customer wallet",
    ),
    ("tier_disabled", 422, "The client has not enabled this tier"),
    ("device_not_active", 409, "The device is not active"),
    (
        "certificate_expired",
        409,
        "The device certificate has expired",
    ),
    (
        "voucher_open",
        409,
        "The device already has an open voucher",
    ),
    (
        "over_tier_limit",
        422,
        "The amount is over the tier's voucher limit",
    ),
    (
        "over_customer_cap",
        422,
        "The customer's open vouchers would exceed the cap",
    ),
];

/// The posting rule codes, by rule number (1001–1012).
pub(super) const POSTING: [&str; 12] = [
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
pub(super) const TEMPLATE: [&str; 22] = [
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_used_is_in_the_catalogue() {
        for code in POSTING.iter().chain(TEMPLATE.iter()) {
            assert!(CATALOGUE.iter().any(|(c, ..)| c == code), "{code}");
        }
    }
}

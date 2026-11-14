# API Error Catalogue

[API](README.md) · Contract: [`openapi/mezgeb.v1.yaml`](../../openapi/mezgeb.v1.yaml)

Every error is an RFC 9457 Problem Details document (`application/problem+json`)
with a stable `code`. Clients match on `code`, never on `title` or `detail`,
which may be reworded. A code is never renumbered, reused or removed; a new
rule gets a new code. Posting and template rules also carry their numeric
`rule` (`crates/core/src/error.rs`, `crates/ops/src/error.rs`), and fields
such as `line`, `role` or `name` that say exactly what was at fault.

The contract has a worked example of every code (`components/examples`),
and a test (`crates/api/tests/openapi.rs`) fails if a posting or template
rule has no code here.

```json
{
  "type": "urn:mezgeb:problem:insufficient_funds",
  "title": "An account would go below zero",
  "status": 422,
  "code": "insufficient_funds",
  "rule": 1012,
  "detail": "Line 0 would take account 0192f3c4-0000-7000-8000-000000000002 below zero.",
  "line": 0,
  "instance": "urn:uuid:0192f3c4-0000-7000-8000-00000000abcd"
}
```

## Request and protocol

| Code | HTTP | Rule | Meaning | Source |
|---|---:|---:|---|---|
| `invalid_request` | 400 |  | The request is malformed | request: bad JSON, an unknown field, a bad UUID, amount, currency or time |
| `idempotency_key_required` | 400 |  | An Idempotency-Key header is required | request: a write without Idempotency-Key |
| `unauthenticated` | 401 |  | Authentication is required | step 2.08: no token, or an invalid or expired one |
| `forbidden` | 403 |  | The caller may not do this | step 2.08: the token lacks the scope |
| `not_found` | 404 |  | Not found | an account, entry, hold, period or adjustment that does not exist, or that the caller may not see |
| `idempotency_key_reused` | 409 |  | The idempotency key was used for a different request | LedgerError::IdempotencyKeyReused |
| `rate_limited` | 429 |  | Too many requests | API gateway: per-client rate limit |
| `internal_error` | 500 |  | Internal error | LedgerError::Storage, or a template defect (TemplateError 2013) |
| `not_implemented` | 501 |  | Not implemented yet | offline endpoints, before steps 2.11 and 2.12 |
| `busy` | 503 |  | The accounts are busy; retry later | LedgerError::Busy (Retry-After) |
| `contention` | 503 |  | Conflicted with concurrent operations; retry later | LedgerError::Contention (Retry-After) |
| `no_open_period` | 503 |  | No accounting period is open | LedgerError::NoOpenPeriod |

## Posting rules

| Code | HTTP | Rule | Meaning | Source |
|---|---:|---:|---|---|
| `too_few_lines` | 422 | 1001 | An entry needs at least 2 lines | PostingError::TooFewLines |
| `too_many_lines` | 422 | 1002 | An entry has too many lines | PostingError::TooManyLines |
| `metadata_too_large` | 422 | 1003 | The metadata is too large | PostingError::MetadataTooLarge |
| `amount_not_positive` | 422 | 1004 | A line amount is not positive | PostingError::AmountNotPositive |
| `account_not_found` | 422 | 1005 | A line names an account that does not exist | PostingError::AccountNotFound |
| `account_not_open` | 422 | 1006 | An account is not open | PostingError::AccountNotOpen |
| `currency_mismatch` | 422 | 1007 | A line's currency is not its account's | PostingError::CurrencyMismatch |
| `duplicate_line` | 422 | 1008 | Two lines share an account and side | PostingError::DuplicateLine |
| `amount_overflow` | 422 | 1009 | The amounts are too large | PostingError::AmountOverflow |
| `unbalanced` | 422 | 1010 | Debits and credits differ | PostingError::Unbalanced |
| `account_total_overflow` | 422 | 1011 | An account's totals would be too large | PostingError::AccountTotalOverflow |
| `insufficient_funds` | 422 | 1012 | An account would go below zero | PostingError::InsufficientFunds |

## Template rules

| Code | HTTP | Rule | Meaning | Source |
|---|---:|---:|---|---|
| `invalid_template_id` | 404 | 2001 | The template ID is malformed | TemplateError::InvalidTemplateId |
| `unknown_template` | 404 | 2002 | No such template | TemplateError::UnknownTemplate |
| `unknown_template_version` | 404 | 2003 | No such template version | TemplateError::UnknownVersion |
| `missing_amount` | 422 | 2004 | An amount the template needs is missing | TemplateError::MissingAmount |
| `unexpected_amount` | 422 | 2005 | The template does not use this amount | TemplateError::UnexpectedAmount |
| `non_positive_amount` | 422 | 2006 | An amount is not positive | TemplateError::AmountNotPositive |
| `missing_account` | 422 | 2007 | A role the template books has no account | TemplateError::MissingAccount |
| `unexpected_account` | 422 | 2008 | The template has no such role | TemplateError::UnexpectedAccount |
| `bound_account_not_found` | 422 | 2009 | The account bound to a role does not exist | TemplateError::AccountNotFound |
| `account_not_allowed` | 422 | 2010 | The account is not allowed for its role | TemplateError::AccountNotAllowed |
| `same_account` | 422 | 2011 | Two roles are bound to the same account | TemplateError::SameAccount |
| `reserved_metadata` | 422 | 2012 | The metadata sets a reserved key | TemplateError::ReservedMetadata |
| `template_defect` | 500 | 2013 | The template books a role it does not declare | TemplateError::UndeclaredRole |
| `amount_not_whole_balance` | 422 | 2014 | The amount is not the item's whole balance | TemplateError::AmountNotBalance |
| `account_not_new` | 422 | 2015 | The item account has already been used | TemplateError::AccountNotNew |
| `fee_exceeds_amount` | 422 | 2016 | The fees are more than the amount | TemplateError::FeeExceedsAmount |
| `amount_currency_mismatch` | 422 | 2017 | The amounts are in different currencies | TemplateError::CurrencyMismatch |
| `amount_too_large` | 422 | 2018 | An amount is too large | TemplateError::AmountOverflow |
| `missing_metadata` | 422 | 2019 | A metadata key the template requires is missing | TemplateError::MissingMetadata |
| `invalid_metadata` | 422 | 2020 | A metadata value is not allowed | TemplateError::InvalidMetadata |
| `missing_refunded_entry` | 422 | 2021 | A refund must name the entry it refunds | TemplateError::MissingRefundedEntry |
| `unexpected_refunded_entry` | 422 | 2022 | Only a refund template names an entry to refund | TemplateError::UnexpectedRefundedEntry |

## Ledger operations

| Code | HTTP | Rule | Meaning | Source |
|---|---:|---:|---|---|
| `invalid_reference` | 422 |  | The chart reference is invalid or not open for accounts | LedgerError::Chart |
| `duplicate_reference` | 409 |  | An account already exists for this reference | LedgerError::DuplicateReference |
| `invalid_status_change` | 409 |  | The account cannot change to that status | LedgerError::Status |
| `already_reversed` | 409 |  | The entry is already reversed | LedgerError::AlreadyReversed |
| `cannot_reverse_reversal` | 409 |  | A reversal cannot be reversed | LedgerError::CannotReverseReversal |
| `reason_not_allowed` | 422 |  | The reason is unknown or not for this kind of entry | LedgerError::ReasonNotAllowed |
| `maker_is_checker` | 403 |  | The approver must be a different person | LedgerError::MakerIsChecker |
| `hold_not_active` | 409 |  | The hold has already ended | LedgerError::HoldNotActive |
| `hold_expired` | 409 |  | The hold has expired | LedgerError::HoldExpired |
| `capture_exceeds_hold` | 422 |  | The capture is larger than the hold | LedgerError::CaptureExceedsHold |
| `invalid_held_account` | 422 |  | The held account must be a different, unused account of the same type | LedgerError::InvalidHeldAccount |
| `account_is_held` | 422 |  | The account is a hold's held account | LedgerError::AccountIsHeld |
| `not_refundable` | 409 |  | Only a standard entry can be refunded | LedgerError::NotRefundable |
| `refund_of_reversed_entry` | 409 |  | The entry has been reversed | LedgerError::RefundOfReversedEntry |
| `refund_line_not_allowed` | 422 |  | The refund moves an account the original did not move that way | LedgerError::RefundLineNotAllowed |
| `refund_exceeds_original` | 422 |  | The refunds would give back more than the original moved | LedgerError::RefundExceedsOriginal |
| `refunded_entry_cannot_be_reversed` | 409 |  | The entry has refunds | LedgerError::RefundedEntryCannotBeReversed |
| `cannot_reverse_refund` | 409 |  | A refund cannot be reversed | LedgerError::CannotReverseRefund |
| `period_not_closed` | 409 |  | The period is not closed | LedgerError::PeriodNotClosed |
| `period_label_taken` | 409 |  | A period with this label exists | the periods_label_key constraint |
| `period_close_findings` | 409 |  | The period did not close: verification found problems | CloseReport with findings |
| `adjustment_not_pending` | 409 |  | The adjustment request is no longer pending | API: approval or rejection of a decided request |

## Retrying

- `busy`, `contention` and `no_open_period` (503) and `rate_limited` (429)
  are safe to retry after `Retry-After`, with the same `Idempotency-Key`.
- Any other 4xx will fail the same way again: fix the request.
- A 500 may or may not have posted: retry with the same `Idempotency-Key`,
  or look the entry up by its key (`GET /v1/entries/by-idempotency-key/{key}`).

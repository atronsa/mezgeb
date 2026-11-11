//! Posting errors with stable numeric codes.
//!
//! The codes are part of the public API: clients match on them. A code is
//! never renumbered or reused; a new rule gets the next free number.
//!
//! | Code | Error | Rule |
//! |---:|---|---|
//! | 1001 | `TooFewLines` | An entry has at least 2 lines |
//! | 1002 | `TooManyLines` | An entry has at most [`MAX_LINES`] lines |
//! | 1003 | `MetadataTooLarge` | At most [`MAX_METADATA_KEYS`] keys and [`MAX_METADATA_BYTES`] bytes |
//! | 1004 | `AmountNotPositive` | Every line amount is greater than zero |
//! | 1005 | `AccountNotFound` | Every account exists |
//! | 1006 | `AccountNotOpen` | Every account is open |
//! | 1007 | `CurrencyMismatch` | A line's currency is its account's currency |
//! | 1008 | `DuplicateLine` | No two lines share an account and side, unless allowed |
//! | 1009 | `AmountOverflow` | Each currency's debits and credits fit in an `i64` |
//! | 1010 | `Unbalanced` | Debits equal credits in every currency |
//! | 1011 | `AccountTotalOverflow` | Each account's totals still fit in an `i64` after the entry |
//! | 1012 | `InsufficientFunds` | No account goes below zero on its normal side, unless it allows it (INV-8) |
//!
//! Rules are checked in this order, and lines in their order in the entry,
//! so the same invalid entry always gets the same error. Rules 1011 and 1012
//! are checked per account, in the order accounts first appear, and report
//! the first line that touches the account.
//!
//! [`MAX_LINES`]: crate::entry::MAX_LINES
//! [`MAX_METADATA_KEYS`]: crate::entry::MAX_METADATA_KEYS
//! [`MAX_METADATA_BYTES`]: crate::entry::MAX_METADATA_BYTES

use std::fmt;

use crate::account::AccountStatus;
use crate::currency::Currency;

/// Why an entry was refused. `line` is the zero-based index of the first
/// offending line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostingError {
    TooFewLines,
    TooManyLines,
    MetadataTooLarge,
    AmountNotPositive {
        line: usize,
    },
    AccountNotFound {
        line: usize,
    },
    AccountNotOpen {
        line: usize,
        status: AccountStatus,
    },
    CurrencyMismatch {
        line: usize,
        expected: Currency,
        found: Currency,
    },
    DuplicateLine {
        line: usize,
    },
    AmountOverflow {
        currency: Currency,
    },
    Unbalanced {
        currency: Currency,
        debits: i64,
        credits: i64,
    },
    AccountTotalOverflow {
        line: usize,
    },
    InsufficientFunds {
        line: usize,
    },
}

impl PostingError {
    /// The stable numeric code.
    pub const fn code(&self) -> u16 {
        match self {
            PostingError::TooFewLines => 1001,
            PostingError::TooManyLines => 1002,
            PostingError::MetadataTooLarge => 1003,
            PostingError::AmountNotPositive { .. } => 1004,
            PostingError::AccountNotFound { .. } => 1005,
            PostingError::AccountNotOpen { .. } => 1006,
            PostingError::CurrencyMismatch { .. } => 1007,
            PostingError::DuplicateLine { .. } => 1008,
            PostingError::AmountOverflow { .. } => 1009,
            PostingError::Unbalanced { .. } => 1010,
            PostingError::AccountTotalOverflow { .. } => 1011,
            PostingError::InsufficientFunds { .. } => 1012,
        }
    }
}

impl fmt::Display for PostingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code(), Message(self))
    }
}

/// The message without the code.
struct Message<'a>(&'a PostingError);

impl fmt::Display for Message<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            PostingError::TooFewLines => f.write_str("an entry needs at least 2 lines"),
            PostingError::TooManyLines => write!(
                f,
                "an entry may have at most {} lines",
                crate::entry::MAX_LINES
            ),
            PostingError::MetadataTooLarge => f.write_str("entry metadata is too large"),
            PostingError::AmountNotPositive { line } => {
                write!(f, "line {line}: amount must be greater than zero")
            }
            PostingError::AccountNotFound { line } => write!(f, "line {line}: account not found"),
            PostingError::AccountNotOpen { line, status } => {
                write!(f, "line {line}: account is {status}")
            }
            PostingError::CurrencyMismatch {
                line,
                expected,
                found,
            } => write!(
                f,
                "line {line}: account is in {expected}, line is in {found}"
            ),
            PostingError::DuplicateLine { line } => {
                write!(f, "line {line}: same account and side as an earlier line")
            }
            PostingError::AmountOverflow { currency } => {
                write!(f, "{currency} amounts in this entry are too large")
            }
            PostingError::Unbalanced {
                currency,
                debits,
                credits,
            } => write!(
                f,
                "{currency} debits ({debits}) do not equal credits ({credits})"
            ),
            PostingError::AccountTotalOverflow { line } => {
                write!(f, "line {line}: account totals would overflow")
            }
            PostingError::InsufficientFunds { line } => {
                write!(f, "line {line}: account would go below zero")
            }
        }
    }
}

impl std::error::Error for PostingError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins every code. If this test fails, a code was renumbered: revert
    /// it, and give a new rule the next free number instead.
    #[test]
    fn codes_never_change() {
        let etb = Currency::ETB;
        let cases = [
            (
                PostingError::TooFewLines,
                1001,
                "[1001] an entry needs at least 2 lines",
            ),
            (
                PostingError::TooManyLines,
                1002,
                "[1002] an entry may have at most 100 lines",
            ),
            (
                PostingError::MetadataTooLarge,
                1003,
                "[1003] entry metadata is too large",
            ),
            (
                PostingError::AmountNotPositive { line: 1 },
                1004,
                "[1004] line 1: amount must be greater than zero",
            ),
            (
                PostingError::AccountNotFound { line: 0 },
                1005,
                "[1005] line 0: account not found",
            ),
            (
                PostingError::AccountNotOpen {
                    line: 2,
                    status: AccountStatus::Frozen,
                },
                1006,
                "[1006] line 2: account is frozen",
            ),
            (
                PostingError::CurrencyMismatch {
                    line: 0,
                    expected: etb,
                    found: Currency::USD,
                },
                1007,
                "[1007] line 0: account is in ETB, line is in USD",
            ),
            (
                PostingError::DuplicateLine { line: 3 },
                1008,
                "[1008] line 3: same account and side as an earlier line",
            ),
            (
                PostingError::AmountOverflow { currency: etb },
                1009,
                "[1009] ETB amounts in this entry are too large",
            ),
            (
                PostingError::Unbalanced {
                    currency: etb,
                    debits: 100,
                    credits: 99,
                },
                1010,
                "[1010] ETB debits (100) do not equal credits (99)",
            ),
            (
                PostingError::AccountTotalOverflow { line: 0 },
                1011,
                "[1011] line 0: account totals would overflow",
            ),
            (
                PostingError::InsufficientFunds { line: 1 },
                1012,
                "[1012] line 1: account would go below zero",
            ),
        ];
        for (error, code, message) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(error.to_string(), message);
        }
    }
}

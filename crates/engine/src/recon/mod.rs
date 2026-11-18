//! Reconciliation (step 2.15, ADR-0020, migration 0014).
//!
//! Finds every disagreement between the ledger and another record of the
//! same money, and never changes the ledger to remove one:
//!
//! - **External** ([`import_statement`], [`reconcile_account`]): an account,
//!   such as 1010, against the bank's statement lines.
//! - **Internal** ([`reconcile_cache`]): a client's cached balances against
//!   the ledger's, at the same point in posting order.
//! - **Offline** ([`reconcile_offline`]): devices' reported counters and the
//!   escrows against the receipts and entries the ledger holds.
//!
//! Each disagreement is a [`ReconBreak`], open until a later run clears it,
//! an entry resolves it ([`resolve_break`], money breaks only) or an
//! operator explains it ([`explain_break`]). The engine does not know which
//! templates may resolve a break; the caller passes them, as it passes the
//! codes to [`ageing`](crate::ageing).

mod cache;
mod matching;
mod offline;
mod statement;
mod store;

use std::fmt;
use std::str::FromStr;

use mezgeb_core::{AccountId, ClientId, LedgerError, Timestamp};
use uuid::Uuid;

pub use cache::{CacheReport, CacheResult, CachedBalance, reconcile_cache};
pub use matching::{
    Coverage, DuplicateKey, Finding, Item, LedgerLine, MatchOptions, Matching, Rule, match_lines,
};
pub use offline::reconcile_offline;
pub use statement::{
    Columns, DateFormat, Mapping, Sign, StatementError, StatementLine, days_from_civil,
    format_date, parse_date, parse_statement,
};
pub use store::{
    BreakFilter, ImportReport, explain_break, get_break, import_statement, list_breaks, list_runs,
    reconcile_account, resolve_break,
};

/// The entry metadata key under which a client records the bank's line ID
/// when it books a movement of a reconciled account: what the exact pass
/// matches on.
pub const BANK_REF_KEY: &str = "bank_ref";
/// The entry metadata key for the payment reference as the bank shows it:
/// what the rule pass matches on.
pub const PAYMENT_REF_KEY: &str = "payment_ref";

/// What a run reconciled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// A statement imported.
    Import,
    /// An account against its statement lines.
    External,
    /// A client's cached balances.
    Internal,
    /// Devices and escrows.
    Offline,
}

/// What a break is about (ADR-0020 decision 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BreakKind {
    MissingInLedger,
    MissingInStatement,
    AmountMismatch,
    DuplicateStatementLine,
    StatementConflict,
    StaleCache,
    MissingReceipts,
    CounterBehind,
    ReceiptEntryMismatch,
    EscrowMismatch,
}

/// Where a break is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakStatus {
    Open,
    /// A later run matched what it was about.
    Cleared,
    /// An entry resolved it.
    Resolved,
    /// An operator explained it.
    Explained,
}

macro_rules! names {
    ($ty:ident { $($variant:ident => $name:literal),* $(,)? }) => {
        impl $ty {
            /// Every value, in order.
            pub const ALL: &'static [$ty] = &[$($ty::$variant),*];

            /// The name stored and shown, such as `missing_in_ledger`.
            pub fn as_str(self) -> &'static str {
                match self { $($ty::$variant => $name),* }
            }
        }

        impl FromStr for $ty {
            type Err = String;

            fn from_str(s: &str) -> Result<$ty, String> {
                match s {
                    $($name => Ok($ty::$variant),)*
                    other => Err(format!("unknown {}: {other}", stringify!($ty))),
                }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

names!(RunKind {
    Import => "import",
    External => "external",
    Internal => "internal",
    Offline => "offline",
});

names!(BreakKind {
    MissingInLedger => "missing_in_ledger",
    MissingInStatement => "missing_in_statement",
    AmountMismatch => "amount_mismatch",
    DuplicateStatementLine => "duplicate_statement_line",
    StatementConflict => "statement_conflict",
    StaleCache => "stale_cache",
    MissingReceipts => "missing_receipts",
    CounterBehind => "counter_behind",
    ReceiptEntryMismatch => "receipt_entry_mismatch",
    EscrowMismatch => "escrow_mismatch",
});

names!(BreakStatus {
    Open => "open",
    Cleared => "cleared",
    Resolved => "resolved",
    Explained => "explained",
});

impl BreakKind {
    /// A break about money in a reconciled account: it closes only when a
    /// later run matches it or an entry resolves it, never by explanation
    /// (ADR-0020 decision 7).
    pub fn is_money(self) -> bool {
        matches!(
            self,
            BreakKind::MissingInLedger
                | BreakKind::MissingInStatement
                | BreakKind::AmountMismatch
                | BreakKind::DuplicateStatementLine
        )
    }
}

/// One run, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconRun {
    pub id: Uuid,
    pub kind: RunKind,
    pub account: Option<AccountId>,
    pub client: Option<ClientId>,
    pub source: String,
    pub as_of: Timestamp,
    /// Statement lines imported (import), or items compared.
    pub items: i32,
    pub matched: i32,
    /// Unmatched, but younger than the grace period.
    pub pending: i32,
    pub breaks_new: i32,
    pub breaks_cleared: i32,
    /// Breaks open in the run's scope once it ended.
    pub breaks_open: i32,
    pub runtime_micros: i64,
    pub recorded_at: Timestamp,
}

/// One break, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconBreak {
    pub id: Uuid,
    pub run_id: Uuid,
    pub kind: BreakKind,
    pub subject: String,
    pub account: Option<AccountId>,
    pub client: Option<ClientId>,
    /// The statement line it is about.
    pub item: Option<Uuid>,
    /// The ledger line it is about: its entry and line number.
    pub line: Option<(Uuid, i16)>,
    pub device: Option<Uuid>,
    pub voucher: Option<Uuid>,
    /// What the other record says, what the ledger says, and the
    /// difference (other minus ledger), in minor units.
    pub other_minor: Option<i64>,
    pub ledger_minor: Option<i64>,
    pub difference_minor: Option<i64>,
    /// The currency of the amounts: the account's, or the voucher's.
    pub currency: Option<mezgeb_core::Currency>,
    pub detail: String,
    pub found_at: Timestamp,
    pub status: BreakStatus,
    pub cleared_by_run: Option<Uuid>,
    /// The entry, and its line in the reconciled account, that resolved it.
    pub resolution: Option<(Uuid, i16)>,
    pub resolution_note: Option<String>,
    pub closed_by: Option<ClientId>,
    pub closed_at: Option<Timestamp>,
}

/// Why a reconciliation call was refused.
#[derive(Debug)]
pub enum ReconError {
    /// The request is malformed or names something the ledger refuses.
    Invalid(String),
    /// No such account, break or entry.
    NotFound(String),
    /// The break is no longer open.
    Closed(BreakStatus),
    /// The break cannot close this way, or the entry cannot resolve it.
    Conflict(String),
    Ledger(LedgerError),
}

impl fmt::Display for ReconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReconError::Invalid(m) => write!(f, "invalid: {m}"),
            ReconError::NotFound(m) => write!(f, "not found: {m}"),
            ReconError::Closed(status) => write!(f, "the break is already {status}"),
            ReconError::Conflict(m) => write!(f, "conflict: {m}"),
            ReconError::Ledger(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ReconError {}

impl From<LedgerError> for ReconError {
    fn from(error: LedgerError) -> ReconError {
        ReconError::Ledger(error)
    }
}

impl From<sqlx::Error> for ReconError {
    fn from(error: sqlx::Error) -> ReconError {
        ReconError::Ledger(crate::rows::db(error))
    }
}

/// The day of a timestamp, in days since 1970-01-01 (UTC).
pub(crate) fn day_of(time: Timestamp) -> i32 {
    i32::try_from(time.as_micros().div_euclid(86_400_000_000)).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for kind in BreakKind::ALL {
            assert_eq!(kind.as_str().parse::<BreakKind>(), Ok(*kind));
        }
        for status in BreakStatus::ALL {
            assert_eq!(status.as_str().parse::<BreakStatus>(), Ok(*status));
        }
        for kind in RunKind::ALL {
            assert_eq!(kind.as_str().parse::<RunKind>(), Ok(*kind));
        }
        assert!("other".parse::<BreakKind>().is_err());
    }

    #[test]
    fn days_are_utc() {
        assert_eq!(day_of(Timestamp::from_micros(0)), 0);
        assert_eq!(day_of(Timestamp::from_micros(86_400_000_000 - 1)), 0);
        assert_eq!(day_of(Timestamp::from_micros(-1)), -1);
    }
}

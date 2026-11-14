//! Pure double-entry accounting core.
//!
//! No I/O, no clocks, no randomness: given the same inputs, the result is
//! always the same. That property is what lets `mezgeb-sim` test the ledger
//! deterministically. See `docs/spec.md` and `docs/adr/`.
//!
//! - [`money`], [`currency`], [`ids`], [`time`]: the value types.
//! - [`chart`], [`account`]: the chart of accounts and accounts.
//! - [`entry`], [`validate`], [`error`]: journal entries and posting rules.
//!   A [`BalancedEntry`] can only be created by [`validate`].
//! - [`refund`]: what a refund may give back (INV-15).
//! - [`model`]: the in-memory reference ledger and invariant checker.

pub mod account;
pub mod authorization;
pub mod chart;
pub mod currency;
pub mod entry;
pub mod error;
pub mod fingerprint;
pub mod hold;
pub mod ids;
pub mod model;
pub mod money;
pub mod refund;
pub mod time;
pub mod validate;

pub use account::{Account, AccountRecord, AccountStatus, RecordError, StatusError, balance};
pub use authorization::{ActorId, Authorization, EntryKind, NameError, REASON_CODES, ReasonCode};
pub use chart::{
    AccountCode, AccountKind, AccountTemplate, AccountType, Chart, ChartError, ChartStatus, Side,
};
pub use currency::{Currency, UnknownCurrency};
pub use entry::{AccountsView, BalancedEntry, EntryDraft, Line, LineDraft};
pub use error::PostingError;
pub use fingerprint::{
    Fingerprint, adjustment_fingerprint, hold_capture_fingerprint, hold_end_fingerprint,
    hold_place_fingerprint, reversal_fingerprint,
};
pub use hold::{Hold, HoldStatus};
pub use ids::{AccountId, ClientId, EntryId, HoldId, IdError, IdempotencyKey, IdempotencyKeyError};
pub use model::{
    Holds, InvariantViolation, Ledger, LedgerError, MemLedger, PostedEntry, PostedLine,
    check_invariants,
};
pub use money::{Money, MoneyError, ParseMoneyError, PositiveAmount};
pub use refund::RefundLimits;
pub use time::Timestamp;
pub use validate::validate;

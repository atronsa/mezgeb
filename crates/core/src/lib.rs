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
//! - [`model`]: the in-memory reference ledger and invariant checker.

pub mod account;
pub mod chart;
pub mod currency;
pub mod entry;
pub mod error;
pub mod ids;
pub mod model;
pub mod money;
pub mod time;
pub mod validate;

pub use account::{Account, AccountStatus, StatusError, balance};
pub use chart::{
    AccountCode, AccountKind, AccountTemplate, AccountType, Chart, ChartError, ChartStatus, Side,
};
pub use currency::{Currency, UnknownCurrency};
pub use entry::{AccountsView, BalancedEntry, EntryDraft, Line, LineDraft};
pub use error::PostingError;
pub use ids::{AccountId, ClientId, EntryId, HoldId, IdError, IdempotencyKey, IdempotencyKeyError};
pub use model::{
    InvariantViolation, LedgerError, MemLedger, PostedEntry, PostedLine, check_invariants,
};
pub use money::{Money, MoneyError, ParseMoneyError, PositiveAmount};
pub use time::Timestamp;
pub use validate::validate;

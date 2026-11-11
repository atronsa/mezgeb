//! Pure double-entry accounting engine.
//!
//! No I/O, no clocks, no randomness: given the same state and the same batch,
//! the result is always the same. That property is what lets `mezgeb-sim`
//! test the engine deterministically. See `docs/spec.md`.
//!
//! The `id`, `error`, `transfer` and `state` modules are the draft scaffold
//! and are replaced in step 1.06 (ADR-0003, ADR-0004).

pub mod account;
pub mod chart;
pub mod currency;
pub mod ids;
pub mod money;

pub mod error;
pub mod id;
pub mod state;
pub mod transfer;

pub use account::{Account, AccountStatus, StatusError, balance};
pub use chart::{
    AccountCode, AccountKind, AccountTemplate, AccountType, Chart, ChartError, ChartStatus, Side,
};
pub use currency::{Currency, UnknownCurrency};
pub use ids::{AccountId, ClientId, EntryId, HoldId, IdError, IdempotencyKey, IdempotencyKeyError};
pub use money::{Money, MoneyError, ParseMoneyError, PositiveAmount};

pub use error::{CreateAccountError, CreateTransferError};
pub use id::{Amount, LedgerId, Timestamp, TransferId};
pub use state::{Engine, StateView};
pub use transfer::{Transfer, TransferFlags};

//! Pure double-entry accounting engine.
//!
//! No I/O, no clocks, no randomness: given the same state and the same batch,
//! the result is always the same. That property is what lets `mezgeb-sim`
//! test the engine deterministically. See `docs/spec.md`.

pub mod account;
pub mod error;
pub mod id;
pub mod state;
pub mod transfer;

pub use account::{Account, AccountFlags};
pub use error::{CreateAccountError, CreateTransferError};
pub use id::{AccountId, Amount, LedgerId, Timestamp, TransferId};
pub use state::{Engine, StateView};
pub use transfer::{Transfer, TransferFlags};

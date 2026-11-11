//! The reference model: an in-memory ledger and the invariant checker
//! (step 1.07). Everything later — the PostgreSQL engine, the simulator —
//! is tested by comparing it with this model.

mod api;
mod invariants;
mod ledger;

pub use api::Ledger;
pub use invariants::{InvariantViolation, check_invariants};
pub use ledger::{LedgerError, MemLedger, PostedEntry, PostedLine};

#[cfg(test)]
mod tests;

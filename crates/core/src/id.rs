//! Scaffold scalar types (spec §3.1, §3.2, §3.6).
//!
//! **Scaffold, to be replaced.** Used only by the draft transfer model. New
//! code uses [`crate::ids`] and [`crate::money`]. Removed in steps 1.05–1.06.

/// Client-generated 128-bit transfer ID; the idempotency key. Zero is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(pub u128);

/// Asset/currency partition. Value never moves between ledgers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LedgerId(pub u32);

/// Integer minor units (e.g. santim). Transfer amounts must be > 0.
pub type Amount = i64;

/// Nanoseconds since the Unix epoch. Supplied to the engine, never read
/// from a clock inside it.
pub type Timestamp = u64;

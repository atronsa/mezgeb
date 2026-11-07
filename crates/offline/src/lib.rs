//! Offline sync (spec §5): vouchers reserve funds online, devices spend them
//! offline with signed, hash-chained receipts, and the ledger settles each
//! receipt exactly once no matter who uploads it or when.

pub mod ingest;
pub mod receipt;
pub mod voucher;

pub use ingest::{IngestOutcome, ingest_receipt};
pub use receipt::Receipt;
pub use voucher::{TrustTier, Voucher};

//! Offline sync (spec §5, protocol v1 in `docs/offline/protocol.md`):
//! vouchers reserve funds online, devices spend them offline with signed,
//! hash-chained receipts, and the ledger settles each receipt exactly once
//! no matter who uploads it or when.
//!
//! - [`encoding`]: the canonical bytes of every signed message.
//! - [`crypto`]: P-256 keys and low-S signatures.
//! - [`Tier`]: trust tiers and their limits.
//! - [`DeviceCertificate`] and [`Voucher`]: what the issuer key signs.

pub mod crypto;
mod device;
pub mod encoding;
pub mod ingest;
pub mod receipt;
mod tier;
mod voucher;

pub use crypto::{CryptoError, PublicKey, SecretKey};
pub use device::DeviceCertificate;
pub use ingest::{IngestOutcome, ingest_receipt};
pub use receipt::Receipt;
pub use tier::{
    CERTIFICATE_MICROS, CUSTOMER_CAP_MINOR, GRACE_MICROS, Tier, TierLimits, UnknownTier,
};
pub use voucher::Voucher;

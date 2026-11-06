//! Receipt ingestion and settlement rules (spec §5.5).

use crate::receipt::Receipt;
use crate::voucher::Voucher;
use mezgeb_core::StateView;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    Settled,
    /// Same (device, counter) and identical content: already settled.
    Duplicate,
    /// An earlier counter is missing; held until the gap fills.
    WaitingForGap { missing_from: u64 },
    /// Same (device, counter) with different content: cloned or tampered
    /// device. Device is frozen.
    Conflict,
    Expired,
    OverLimit,
    BadSignature,
    UnknownDevice,
}

/// Ingest one receipt. Must give the same final state regardless of upload
/// order, uploader (payer or payee), or repetition (INV-9, INV-10, INV-11).
pub fn ingest_receipt<S: StateView>(
    _state: &mut S,
    _voucher: &Voucher,
    _receipt: &Receipt,
) -> IngestOutcome {
    todo!("spec §5.5")
}

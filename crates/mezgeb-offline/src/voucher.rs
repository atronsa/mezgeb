//! Offline allowance issued while online (spec §5.3).

use mezgeb_core::{AccountId, Amount, LedgerId, Timestamp, TransferId};

/// Device trust tier; each tier has its own limit and expiry (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTier {
    /// Secure element (card).
    SecureElement,
    /// Hardware-backed keystore (StrongBox/TEE).
    HardwareKeystore,
    /// Software keys only.
    Software,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voucher {
    pub voucher_id: u128,
    pub device_id: u128,
    pub tier: TrustTier,
    /// The user's spendable account the escrow came from.
    pub account_id: AccountId,
    /// The escrow account holding the reserved funds for this device.
    pub reserved_account_id: AccountId,
    /// The pending transfer that created the reservation.
    pub reservation_transfer_id: TransferId,
    pub ledger: LedgerId,
    pub amount: Amount,
    /// First counter value this voucher authorises.
    pub counter_start: u64,
    pub expires_at: Timestamp,
    /// Device public key the receipts must be signed with.
    pub device_public_key: Vec<u8>,
    /// Issuer signature over all fields above.
    pub issuer_signature: Vec<u8>,
}

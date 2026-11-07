//! Signed proof of one offline payment (spec §5.4).

use mezgeb_core::{AccountId, Amount, Timestamp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub device_id: u128,
    pub voucher_id: u128,
    /// Strictly increasing per device, maintained in secure hardware.
    pub counter: u64,
    /// Hash of the previous receipt from this device (hash chain).
    pub prev_receipt_hash: [u8; 32],
    pub payee_account_id: AccountId,
    pub amount: Amount,
    /// Device-claimed time. Validated against bounds, never trusted for order.
    pub event_time: Timestamp,
    pub payer_signature: Vec<u8>,
    pub payee_ack_signature: Option<Vec<u8>>,
}

impl Receipt {
    /// Canonical hash used for the chain and for conflict detection.
    pub fn hash(&self) -> [u8; 32] {
        todo!("spec §5.4: canonical encoding + hash")
    }
}

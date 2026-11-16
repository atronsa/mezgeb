//! Vouchers (protocol §7): an allowance the ledger reserved in escrow while
//! the device was online, signed by the issuer key.

use mezgeb_core::{AccountId, ClientId, DeviceId, Money, Timestamp, VoucherId};

use crate::crypto::{CryptoError, PublicKey, SIGNATURE_LEN, SecretKey};
use crate::encoding::{Body, digest, tag};

/// The fields of a voucher, in signing order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voucher {
    pub voucher_id: VoucherId,
    /// The device it is bound to.
    pub device_id: DeviceId,
    pub client_id: ClientId,
    /// Its escrow sub-account, `2050:vch-…`.
    pub escrow_account_id: AccountId,
    /// The allowance.
    pub amount: Money,
    /// The first counter it authorises: 1 for a device's first voucher,
    /// else the last voucher's last counter + 1.
    pub counter_start: u64,
    /// Ledger time.
    pub issued_at: Timestamp,
    /// Ledger time: `issued_at` + the tier's lifetime.
    pub expires_at: Timestamp,
    pub issuer_key_id: String,
}

impl Voucher {
    /// The canonical body: the tag, then every field in order.
    pub fn body(&self) -> Vec<u8> {
        let mut body = Body::new(tag::VOUCHER);
        body.id(self.voucher_id.as_uuid())
            .id(self.device_id.as_uuid())
            .id(self.client_id.as_uuid())
            .id(self.escrow_account_id.as_uuid())
            .amount(self.amount)
            .u64(self.counter_start)
            .i64(self.issued_at.as_micros())
            .i64(self.expires_at.as_micros())
            .text(&self.issuer_key_id);
        body.into_bytes()
    }

    /// The SHA-256 of the body. A device's first receipt chains to it.
    pub fn digest(&self) -> [u8; 32] {
        digest(&self.body())
    }

    /// Signs the voucher with the issuer key it names.
    pub fn sign(&self, issuer: &SecretKey) -> [u8; SIGNATURE_LEN] {
        issuer.sign(&self.body())
    }

    pub fn verify(&self, issuer: &PublicKey, signature: &[u8]) -> Result<(), CryptoError> {
        issuer.verify(&self.body(), signature)
    }
}

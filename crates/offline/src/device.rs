//! Device certificates (protocol §5).
//!
//! The ledger signs one for each registered device with its current issuer
//! key. The device shows it to a payee with each payment, so the payee can
//! check the device's key and tier offline.

use mezgeb_core::{AccountId, ClientId, DeviceId, Timestamp};

use crate::crypto::{CryptoError, PublicKey, SIGNATURE_LEN, SecretKey};
use crate::encoding::{Body, digest, tag};
use crate::tier::Tier;

/// The fields of a device certificate, in signing order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCertificate {
    pub device_id: DeviceId,
    pub client_id: ClientId,
    pub owner_account_id: AccountId,
    pub public_key: PublicKey,
    pub tier: Tier,
    pub issued_at: Timestamp,
    pub valid_until: Timestamp,
    pub issuer_key_id: String,
}

impl DeviceCertificate {
    /// The canonical body: the tag, then every field in order.
    pub fn body(&self) -> Vec<u8> {
        let mut body = Body::new(tag::DEVICE);
        body.id(self.device_id.as_uuid())
            .id(self.client_id.as_uuid())
            .id(self.owner_account_id.as_uuid())
            .bytes(self.public_key.compressed())
            .text(&self.public_key.key_id())
            .u8(self.tier.code())
            .i64(self.issued_at.as_micros())
            .i64(self.valid_until.as_micros())
            .text(&self.issuer_key_id);
        body.into_bytes()
    }

    pub fn digest(&self) -> [u8; 32] {
        digest(&self.body())
    }

    /// Signs the certificate with the issuer key it names.
    pub fn sign(&self, issuer: &SecretKey) -> [u8; SIGNATURE_LEN] {
        issuer.sign(&self.body())
    }

    pub fn verify(&self, issuer: &PublicKey, signature: &[u8]) -> Result<(), CryptoError> {
        issuer.verify(&self.body(), signature)
    }
}

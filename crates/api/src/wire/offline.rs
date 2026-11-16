//! Offline devices, vouchers and issuer keys. Keys, signed bodies and
//! signatures are lowercase hex: a device checks the signature over the
//! body's exact bytes (protocol §3).

use mezgeb_core::chain::hex;
use mezgeb_engine::{Device, IssuedVoucher};
use serde::Serialize;

use super::*;

#[derive(Debug, Serialize)]
pub struct CertificateJson {
    /// The canonical body the issuer signed, in hex.
    pub body: String,
    pub signature: String,
    pub issuer_key_id: String,
    pub issued_at: String,
    pub valid_until: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceJson {
    pub id: String,
    pub owner_account: String,
    pub public_key: String,
    pub key_id: String,
    pub tier: &'static str,
    pub attestation: &'static str,
    pub label: String,
    pub status: &'static str,
    pub status_reason: Option<String>,
    pub next_counter: u64,
    pub certificate: CertificateJson,
}

impl From<&Device> for DeviceJson {
    fn from(d: &Device) -> DeviceJson {
        let c = &d.certificate;
        DeviceJson {
            id: c.device_id.to_string(),
            owner_account: c.owner_account_id.to_string(),
            public_key: hex(c.public_key.compressed()),
            key_id: c.public_key.key_id(),
            tier: c.tier.as_str(),
            attestation: d.attestation.as_str(),
            label: d.label.clone(),
            status: d.status.as_str(),
            status_reason: d.status_reason.clone(),
            next_counter: d.next_counter,
            certificate: CertificateJson {
                body: hex(&c.body()),
                signature: hex(&d.certificate_signature),
                issuer_key_id: c.issuer_key_id.clone(),
                issued_at: format_time(c.issued_at),
                valid_until: format_time(c.valid_until),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct VoucherJson {
    pub id: String,
    pub device: String,
    pub owner_account: String,
    pub escrow_account: String,
    pub amount: MoneyJson,
    pub counter_start: u64,
    pub issued_at: String,
    pub expires_at: String,
    pub status: &'static str,
    pub entry: String,
    pub issuer_key_id: String,
    /// The canonical body the issuer signed, in hex.
    pub body: String,
    pub signature: String,
}

impl From<&IssuedVoucher> for VoucherJson {
    fn from(i: &IssuedVoucher) -> VoucherJson {
        let v = &i.voucher;
        VoucherJson {
            id: v.voucher_id.to_string(),
            device: v.device_id.to_string(),
            owner_account: i.owner_account.to_string(),
            escrow_account: v.escrow_account_id.to_string(),
            amount: v.amount.into(),
            counter_start: v.counter_start,
            issued_at: format_time(v.issued_at),
            expires_at: format_time(v.expires_at),
            status: i.status.as_str(),
            entry: i.entry_id.to_string(),
            issuer_key_id: v.issuer_key_id.clone(),
            body: hex(&v.body()),
            signature: hex(&i.signature),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct IssuerKeyJson {
    pub key_id: String,
    pub public_key: String,
    pub added_at: String,
}

/// Bytes from lowercase or uppercase hex, or `None`.
pub fn parse_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

//! Offline devices, vouchers and issuer keys. Keys, signed bodies and
//! signatures are lowercase hex: a device checks the signature over the
//! body's exact bytes (protocol §3).

use mezgeb_core::chain::hex;
use mezgeb_engine::{Device, Dispute, IngestResult, IssuedVoucher, ReceiptRecord, Shortfall};
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

#[derive(Debug, Serialize)]
pub struct SettledJson {
    pub counter: u64,
    pub entry: String,
}

/// One uploaded receipt's outcome.
#[derive(Debug, Serialize)]
pub struct ReceiptResultJson {
    /// Its place in the upload.
    pub index: usize,
    pub outcome: &'static str,
    pub device: Option<String>,
    pub counter: Option<u64>,
    pub digest: Option<String>,
    pub entry: Option<String>,
    pub detail: Option<String>,
    /// Every receipt this upload settled: this one, and those that were
    /// waiting for it.
    pub settled: Vec<SettledJson>,
}

impl From<(usize, &IngestResult)> for ReceiptResultJson {
    fn from((index, r): (usize, &IngestResult)) -> ReceiptResultJson {
        ReceiptResultJson {
            index,
            outcome: r.outcome.as_str(),
            device: r.device.map(|d| d.to_string()),
            counter: r.counter,
            digest: r.digest.as_ref().map(|d| hex(d)),
            entry: r.entry.map(|e| e.to_string()),
            detail: r.detail.clone(),
            settled: r
                .settled
                .iter()
                .map(|s| SettledJson {
                    counter: s.counter,
                    entry: s.entry.to_string(),
                })
                .collect(),
        }
    }
}

/// A recorded receipt.
#[derive(Debug, Serialize)]
pub struct OfflineReceiptJson {
    pub device: String,
    pub counter: u64,
    pub voucher: String,
    pub digest: String,
    pub payee: String,
    pub amount: MoneyJson,
    pub event_time: String,
    pub outcome: &'static str,
    pub entry: Option<String>,
}

impl From<&ReceiptRecord> for OfflineReceiptJson {
    fn from(r: &ReceiptRecord) -> OfflineReceiptJson {
        OfflineReceiptJson {
            device: r.device.to_string(),
            counter: r.counter,
            voucher: r.voucher.to_string(),
            digest: hex(&r.digest),
            payee: r.payee.to_string(),
            amount: r.amount.into(),
            event_time: format_time(r.event_time),
            outcome: r.outcome.as_str(),
            entry: r.entry.map(|e| e.to_string()),
        }
    }
}

/// What a late receipt's payer could not pay, paid by the loss account.
#[derive(Debug, Serialize)]
pub struct ShortfallJson {
    pub id: String,
    pub owner_account: String,
    pub device: String,
    pub counter: u64,
    pub amount: MoneyJson,
    pub entry: String,
    pub repaid_entry: Option<String>,
}

impl From<&Shortfall> for ShortfallJson {
    fn from(s: &Shortfall) -> ShortfallJson {
        ShortfallJson {
            id: s.id.to_string(),
            owner_account: s.owner_account.to_string(),
            device: s.device.to_string(),
            counter: s.counter,
            amount: s.amount.into(),
            entry: s.entry.to_string(),
            repaid_entry: s.repaid_entry.map(|e| e.to_string()),
        }
    }
}

/// A frozen voucher's case.
#[derive(Debug, Serialize)]
pub struct DisputeJson {
    pub id: String,
    pub client: String,
    pub device: String,
    pub voucher: String,
    pub reason: String,
    pub counter: u64,
    pub status: &'static str,
    pub resolution: Option<&'static str>,
    pub resolved_by: Option<String>,
}

impl From<&Dispute> for DisputeJson {
    fn from(d: &Dispute) -> DisputeJson {
        DisputeJson {
            id: d.id.to_string(),
            client: d.client.to_string(),
            device: d.device.to_string(),
            voucher: d.voucher.to_string(),
            reason: d.reason.clone(),
            counter: d.counter,
            status: if d.resolution.is_some() {
                "resolved"
            } else {
                "open"
            },
            resolution: d.resolution.map(|r| r.as_str()),
            resolved_by: d.resolved_by.clone(),
        }
    }
}

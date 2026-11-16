//! Receipts (protocol §8): the payer device's signed claim that it paid a
//! payee offline, against its voucher.

use mezgeb_core::{AccountId, DeviceId, Money, Timestamp, VoucherId};

use crate::crypto::{CryptoError, PublicKey, SIGNATURE_LEN, SecretKey};
use crate::encoding::{Body, DecodeError, Reader, digest, tag};

/// The fields of a receipt, in signing order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The payer's device.
    pub device_id: DeviceId,
    pub voucher_id: VoucherId,
    /// Strictly increasing per device; the voucher's first receipt has
    /// `counter_start`.
    pub counter: u64,
    /// The previous receipt's digest; for the voucher's first receipt, the
    /// voucher's digest.
    pub prev_digest: [u8; 32],
    /// Who is paid.
    pub payee_account_id: AccountId,
    /// The payee terminal, if it has one.
    pub payee_device_id: Option<DeviceId>,
    /// In the voucher's currency.
    pub amount: Money,
    /// The device's clock: recorded, never trusted.
    pub event_time: Timestamp,
    /// From the payee, so the receipt is fresh and for this payment.
    pub payee_challenge: [u8; 16],
}

fn bad(what: &str) -> impl Fn(mezgeb_core::IdError) -> DecodeError + '_ {
    move |e| DecodeError(format!("{what}: {e}"))
}

impl Receipt {
    /// The canonical body: the tag, then every field in order.
    pub fn body(&self) -> Vec<u8> {
        let mut body = Body::new(tag::RECEIPT);
        body.id(self.device_id.as_uuid())
            .id(self.voucher_id.as_uuid())
            .u64(self.counter)
            .fixed(&self.prev_digest)
            .id(self.payee_account_id.as_uuid())
            .option(self.payee_device_id, |b, d| {
                b.id(d.as_uuid());
            })
            .amount(self.amount)
            .i64(self.event_time.as_micros())
            .fixed(&self.payee_challenge);
        body.into_bytes()
    }

    /// Reads a body, strictly: the receipt tag, every field, UUIDv7 IDs, a
    /// known currency, nothing after the challenge. The result's
    /// [`body`](Receipt::body) is the same bytes.
    pub fn decode(body: &[u8]) -> Result<Receipt, DecodeError> {
        let mut r = Reader::new(body, tag::RECEIPT)?;
        let receipt = Receipt {
            device_id: DeviceId::from_uuid(r.id("device_id")?).map_err(bad("device_id"))?,
            voucher_id: VoucherId::from_uuid(r.id("voucher_id")?).map_err(bad("voucher_id"))?,
            counter: r.u64("counter")?,
            prev_digest: r.fixed("prev_digest")?,
            payee_account_id: AccountId::from_uuid(r.id("payee_account_id")?)
                .map_err(bad("payee_account_id"))?,
            payee_device_id: r
                .option("payee_device_id", |r| r.id("payee_device_id"))?
                .map(DeviceId::from_uuid)
                .transpose()
                .map_err(bad("payee_device_id"))?,
            amount: r.amount("amount")?,
            event_time: Timestamp::from_micros(r.i64("event_time")?),
            payee_challenge: r.fixed("payee_challenge")?,
        };
        r.finish()?;
        Ok(receipt)
    }

    /// The SHA-256 of the body: what the next receipt chains to, and what
    /// tells a duplicate from a conflict. The signature is not part of it.
    pub fn digest(&self) -> [u8; 32] {
        digest(&self.body())
    }

    /// Signs the receipt with the payer device's key (a test device's here;
    /// real devices sign in their secure hardware).
    pub fn sign(&self, device: &SecretKey) -> [u8; SIGNATURE_LEN] {
        device.sign(&self.body())
    }

    pub fn verify(&self, device: &PublicKey, signature: &[u8]) -> Result<(), CryptoError> {
        device.verify(&self.body(), signature)
    }
}

/// What happened to one uploaded receipt (protocol §9, spec §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// Paid from escrow (E17).
    Settled,
    /// Paid under the loss policy after the voucher returned (E28/E29).
    SettledLate,
    /// Already recorded with the same digest: nothing posted.
    Duplicate,
    /// An earlier counter has not arrived: kept until it does.
    WaitingForGap,
    /// Another receipt has this counter, or the chain link is wrong: the
    /// device and voucher freeze.
    Conflict,
    /// The device or voucher is frozen: kept for the dispute.
    Frozen,
    /// More than the escrow left: the device and voucher freeze.
    OverLimit,
    /// Past the late deadline: nothing posted.
    Expired,
    /// Not a receipt this ledger can read.
    BadFormat,
    /// The signature does not verify with the device's key.
    BadSignature,
    /// No such device for this client.
    UnknownDevice,
    /// Not the device's voucher, or a counter before it.
    UnknownVoucher,
}

impl Outcome {
    pub const ALL: [Outcome; 12] = [
        Outcome::Settled,
        Outcome::SettledLate,
        Outcome::Duplicate,
        Outcome::WaitingForGap,
        Outcome::Conflict,
        Outcome::Frozen,
        Outcome::OverLimit,
        Outcome::Expired,
        Outcome::BadFormat,
        Outcome::BadSignature,
        Outcome::UnknownDevice,
        Outcome::UnknownVoucher,
    ];

    /// As in the protocol and the API: `SETTLED`, `WAITING_FOR_GAP`, ...
    pub const fn as_str(self) -> &'static str {
        match self {
            Outcome::Settled => "SETTLED",
            Outcome::SettledLate => "SETTLED_LATE",
            Outcome::Duplicate => "DUPLICATE",
            Outcome::WaitingForGap => "WAITING_FOR_GAP",
            Outcome::Conflict => "CONFLICT",
            Outcome::Frozen => "FROZEN",
            Outcome::OverLimit => "OVER_LIMIT",
            Outcome::Expired => "EXPIRED",
            Outcome::BadFormat => "BAD_FORMAT",
            Outcome::BadSignature => "BAD_SIGNATURE",
            Outcome::UnknownDevice => "UNKNOWN_DEVICE",
            Outcome::UnknownVoucher => "UNKNOWN_VOUCHER",
        }
    }

    pub fn parse(text: &str) -> Option<Outcome> {
        Outcome::ALL.into_iter().find(|o| o.as_str() == text)
    }
}

#[cfg(test)]
mod tests {
    use mezgeb_core::Currency;

    use super::*;

    fn receipt() -> Receipt {
        let id = |n: u64| format!("019a8f00-0000-7000-8000-{n:012x}");
        Receipt {
            device_id: id(1).parse().expect("an ID"),
            voucher_id: id(4).parse().expect("an ID"),
            counter: 3,
            prev_digest: [7; 32],
            payee_account_id: id(9).parse().expect("an ID"),
            payee_device_id: Some(id(10).parse().expect("an ID")),
            amount: Money::from_minor(6_000, Currency::ETB),
            event_time: Timestamp::from_micros(1_794_787_200_000_000),
            payee_challenge: [5; 16],
        }
    }

    #[test]
    fn a_receipt_decodes_to_itself() {
        let r = receipt();
        assert_eq!(Receipt::decode(&r.body()), Ok(r.clone()));
        let without = Receipt {
            payee_device_id: None,
            ..r
        };
        assert_eq!(Receipt::decode(&without.body()), Ok(without));
    }

    #[test]
    fn anything_but_the_exact_encoding_is_refused() {
        let body = receipt().body();
        // Truncated anywhere, or with a byte added.
        for n in 0..body.len() {
            assert!(Receipt::decode(&body[..n]).is_err(), "cut at {n}");
        }
        let mut longer = body.clone();
        longer.push(0);
        assert!(Receipt::decode(&longer).is_err());
        // Another message's tag.
        let mut voucher_tag = crate::encoding::tag::VOUCHER.to_vec();
        voucher_tag.extend_from_slice(&body[crate::encoding::tag::RECEIPT.len()..]);
        assert!(Receipt::decode(&voucher_tag).is_err());
        // An option flag that is neither 0 nor 1.
        let flag = crate::encoding::tag::RECEIPT.len() + 16 + 16 + 8 + 32 + 16;
        let mut bad_flag = body.clone();
        bad_flag[flag] = 2;
        assert!(Receipt::decode(&bad_flag).is_err());
    }

    #[test]
    fn outcomes_have_one_spelling() {
        for o in Outcome::ALL {
            assert_eq!(Outcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(Outcome::parse("settled"), None);
    }
}

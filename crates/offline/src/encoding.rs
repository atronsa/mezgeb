//! The canonical encoding of offline messages (protocol §3).
//!
//! Every signed message is these bytes and nothing else: no JSON, no
//! serialisation library, so card firmware and phone apps in any language
//! can reproduce them from the protocol and check themselves against the
//! committed test vectors (`test-vectors/offline/`).
//!
//! | Kind | Encoding |
//! |---|---|
//! | Integers | Big-endian `u8`, `u32`, `u64`, `i64` |
//! | IDs | The 16 bytes of the UUID |
//! | Byte strings | A `u32` length, then the bytes |
//! | Text | A `u32` length, then UTF-8 |
//! | Amount | The ISO 4217 code as text, then the minor units as `i64` |
//!
//! A message's **body** is its tag and fields in order; its **digest** is
//! the SHA-256 of the body; its signature is over the body and never part of
//! the digest.

use mezgeb_core::Money;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Every message starts with its tag, so no signature over one kind can pass
/// for another.
pub mod tag {
    pub const ISSUER: &[u8] = b"mezgeb.offline.issuer.v1\0";
    pub const DEVICE: &[u8] = b"mezgeb.offline.device.v1\0";
    pub const VOUCHER: &[u8] = b"mezgeb.offline.voucher.v1\0";
    pub const RECEIPT: &[u8] = b"mezgeb.offline.receipt.v1\0";
    pub const ACK: &[u8] = b"mezgeb.offline.ack.v1\0";
    pub const CLOSE: &[u8] = b"mezgeb.offline.close.v1\0";
}

/// A message body being written.
#[derive(Debug, Clone)]
pub struct Body(Vec<u8>);

impl Body {
    /// A body starting with `tag`.
    pub fn new(tag: &[u8]) -> Body {
        Body(tag.to_vec())
    }

    pub fn u8(&mut self, value: u8) -> &mut Body {
        self.0.push(value);
        self
    }

    pub fn u64(&mut self, value: u64) -> &mut Body {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn i64(&mut self, value: i64) -> &mut Body {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn id(&mut self, value: Uuid) -> &mut Body {
        self.0.extend_from_slice(value.as_bytes());
        self
    }

    /// A `u32` length, then the bytes. Message fields are a few dozen bytes,
    /// far below `u32::MAX`.
    pub fn bytes(&mut self, value: &[u8]) -> &mut Body {
        let len = u32::try_from(value.len()).unwrap_or(u32::MAX);
        self.0.extend_from_slice(&len.to_be_bytes());
        self.0.extend_from_slice(value);
        self
    }

    pub fn text(&mut self, value: &str) -> &mut Body {
        self.bytes(value.as_bytes())
    }

    pub fn amount(&mut self, value: Money) -> &mut Body {
        self.text(value.currency().code());
        self.i64(value.amount())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// The SHA-256 of a body.
pub fn digest(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

#[cfg(test)]
mod tests {
    use mezgeb_core::Currency;

    use super::*;

    #[test]
    fn fields_are_big_endian_and_length_prefixed() {
        let mut body = Body::new(b"t\0");
        body.u8(3)
            .u64(1)
            .i64(-2)
            .text("hi")
            .amount(Money::from_minor(150, Currency::ETB));
        let expected: Vec<u8> = [
            &b"t\0"[..],
            &[3],
            &[0, 0, 0, 0, 0, 0, 0, 1],
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe],
            &[0, 0, 0, 2],
            b"hi",
            &[0, 0, 0, 3],
            b"ETB",
            &[0, 0, 0, 0, 0, 0, 0, 150],
        ]
        .concat();
        assert_eq!(body.into_bytes(), expected);
    }

    #[test]
    fn tags_are_distinct_and_end_in_nul() {
        let tags = [
            tag::ISSUER,
            tag::DEVICE,
            tag::VOUCHER,
            tag::RECEIPT,
            tag::ACK,
            tag::CLOSE,
        ];
        for (i, a) in tags.iter().enumerate() {
            assert_eq!(a.last(), Some(&0));
            for b in &tags[i + 1..] {
                assert!(!a.starts_with(b) && !b.starts_with(a));
            }
        }
    }
}

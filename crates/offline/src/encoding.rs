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

    /// Exactly these bytes, unprefixed: digests and challenges, whose
    /// length the protocol fixes.
    pub fn fixed(&mut self, value: &[u8]) -> &mut Body {
        self.0.extend_from_slice(value);
        self
    }

    /// `0x00`, or `0x01` then the value.
    pub fn option<T>(&mut self, value: Option<T>, write: impl FnOnce(&mut Body, T)) -> &mut Body {
        match value {
            Some(v) => {
                self.0.push(1);
                write(self, v);
            }
            None => self.0.push(0),
        }
        self
    }

    pub fn amount(&mut self, value: Money) -> &mut Body {
        self.text(value.currency().code());
        self.i64(value.amount())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Why a body could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

/// Reads a body written by [`Body`], strictly: the exact tag, every field,
/// and nothing after the last one, so each message has one encoding.
#[derive(Debug)]
pub struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader past `tag`, or an error if the body does not start with it.
    pub fn new(body: &'a [u8], tag: &[u8]) -> Result<Reader<'a>, DecodeError> {
        body.strip_prefix(tag)
            .map(|rest| Reader { rest })
            .ok_or_else(|| DecodeError("not this kind of message, or another version".into()))
    }

    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], DecodeError> {
        if self.rest.len() < n {
            return Err(DecodeError(format!("the body ends inside {what}")));
        }
        let (head, rest) = self.rest.split_at(n);
        self.rest = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self, what: &str) -> Result<[u8; N], DecodeError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N, what)?);
        Ok(out)
    }

    pub fn u8(&mut self, what: &str) -> Result<u8, DecodeError> {
        Ok(self.array::<1>(what)?[0])
    }

    pub fn u64(&mut self, what: &str) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.array(what)?))
    }

    pub fn i64(&mut self, what: &str) -> Result<i64, DecodeError> {
        Ok(i64::from_be_bytes(self.array(what)?))
    }

    pub fn id(&mut self, what: &str) -> Result<Uuid, DecodeError> {
        Ok(Uuid::from_bytes(self.array(what)?))
    }

    /// Exactly `N` bytes, unprefixed: digests and challenges.
    pub fn fixed<const N: usize>(&mut self, what: &str) -> Result<[u8; N], DecodeError> {
        self.array(what)
    }

    pub fn bytes(&mut self, what: &str) -> Result<&'a [u8], DecodeError> {
        let len = u32::from_be_bytes(self.array(what)?);
        let len = usize::try_from(len).map_err(|_| DecodeError(format!("{what} is too long")))?;
        self.take(len, what)
    }

    pub fn text(&mut self, what: &str) -> Result<&'a str, DecodeError> {
        std::str::from_utf8(self.bytes(what)?)
            .map_err(|_| DecodeError(format!("{what} is not UTF-8")))
    }

    /// `0x00`, or `0x01` then the value.
    pub fn option<T>(
        &mut self,
        what: &str,
        read: impl FnOnce(&mut Reader<'a>) -> Result<T, DecodeError>,
    ) -> Result<Option<T>, DecodeError> {
        match self.u8(what)? {
            0 => Ok(None),
            1 => read(self).map(Some),
            _ => Err(DecodeError(format!("{what} is neither absent nor present"))),
        }
    }

    pub fn amount(&mut self, what: &str) -> Result<Money, DecodeError> {
        let code = self.text(what)?;
        let currency = mezgeb_core::Currency::from_code(code)
            .map_err(|_| DecodeError(format!("{what}: unknown currency {code:?}")))?;
        Ok(Money::from_minor(self.i64(what)?, currency))
    }

    /// Fails if anything is left.
    pub fn finish(self) -> Result<(), DecodeError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(DecodeError(format!(
                "{} bytes after the last field",
                self.rest.len()
            )))
        }
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

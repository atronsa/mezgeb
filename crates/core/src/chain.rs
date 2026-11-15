//! The hash chains of tamper evidence (ADR-0018, INV-11): what is hashed,
//! byte for byte.
//!
//! Each link is the SHA-256 of a fixed, versioned encoding of one record
//! and the previous link's hash, so changing any byte of any record, or
//! removing one, changes every hash after it. Nothing here is JSON: the
//! bytes never depend on a serialisation library, and a verifier in another
//! language can reproduce them from this description.
//!
//! Encoding: integers are big-endian; text is a 4-byte length then UTF-8;
//! an optional value is a 0 byte, or a 1 byte and the value; IDs are their
//! 16 bytes. Each kind of record starts with its own version tag, and each
//! signed message with another, so no hash or signature of one kind can be
//! taken for another.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::chart::Side;
use crate::ids::{ClientId, EntryId};
use crate::model::PostedEntry;
use crate::time::Timestamp;

/// A link's hash.
pub type ChainHash = [u8; 32];

/// The "previous hash" of the first link of a chain.
pub const GENESIS: ChainHash = [0; 32];

const ENTRY: &[u8] = b"mezgeb.chain.entry.v1\0";
const AUDIT: &[u8] = b"mezgeb.chain.audit.v1\0";
const RECEIPT: &[u8] = b"mezgeb.receipt.v1\0";
const CHECKPOINT: &[u8] = b"mezgeb.checkpoint.v1\0";

struct Encoder(Sha256);

impl Encoder {
    fn new(tag: &[u8]) -> Encoder {
        let mut hasher = Sha256::new();
        hasher.update(tag);
        Encoder(hasher)
    }

    fn bytes(&mut self, value: &[u8]) {
        self.0.update(value);
    }

    fn u64(&mut self, value: u64) {
        self.0.update(value.to_be_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.0.update(value.to_be_bytes());
    }

    fn text(&mut self, value: &str) {
        // Lengths are bounded by the posting and audit limits, far below
        // u32::MAX.
        self.0
            .update(u32::try_from(value.len()).unwrap_or(u32::MAX).to_be_bytes());
        self.0.update(value.as_bytes());
    }

    fn option<T>(&mut self, value: Option<T>, mut write: impl FnMut(&mut Encoder, T)) {
        match value {
            Some(v) => {
                self.0.update([1]);
                write(self, v);
            }
            None => self.0.update([0]),
        }
    }

    fn map(&mut self, map: &BTreeMap<String, String>) {
        self.u64(map.len() as u64);
        for (key, value) in map {
            self.text(key);
            self.text(value);
        }
    }

    fn finish(self) -> ChainHash {
        self.0.finalize().into()
    }
}

/// An entry as the chain records it: the posted entry and what the
/// database assigned to it.
#[derive(Debug, Clone, Copy)]
pub struct ChainEntry<'a> {
    /// Its place in the chain, from 1, in commit order.
    pub position: u64,
    pub entry: &'a PostedEntry,
    pub client: ClientId,
    pub idempotency_key: &'a str,
    pub recorded_at: Timestamp,
    pub period: Option<i64>,
}

/// The hash of an entry's link.
pub fn entry_hash(previous: &ChainHash, link: &ChainEntry<'_>) -> ChainHash {
    let e = link.entry;
    let mut out = Encoder::new(ENTRY);
    out.bytes(previous);
    out.u64(link.position);
    out.bytes(e.id().as_uuid().as_bytes());
    out.u64(e.sequence());
    out.bytes(link.client.as_uuid().as_bytes());
    out.text(link.idempotency_key);
    out.text(e.kind().as_str());
    out.i64(link.recorded_at.as_micros());
    out.option(e.event_time(), |o, t| o.i64(t.as_micros()));
    out.option(link.period, |o, p| o.i64(p));
    out.option(e.reverses(), |o, id: EntryId| {
        o.bytes(id.as_uuid().as_bytes())
    });
    out.option(e.refunds(), |o, id: EntryId| {
        o.bytes(id.as_uuid().as_bytes())
    });
    out.option(e.authorization(), |o, a| {
        o.text(a.reason.as_str());
        o.text(a.requested_by.as_str());
        o.option(a.approved_by.as_ref(), |o, p| o.text(p.as_str()));
    });
    out.map(e.metadata());
    out.u64(e.lines().len() as u64);
    for line in e.lines() {
        out.bytes(line.account().as_uuid().as_bytes());
        out.bytes(&[match line.side() {
            Side::Debit => 1,
            Side::Credit => 2,
        }]);
        out.text(line.amount().currency().code());
        out.i64(line.amount().amount());
        out.i64(line.debits_after());
        out.i64(line.credits_after());
    }
    out.finish()
}

/// An audit event as its chain records it.
#[derive(Debug, Clone, Copy)]
pub struct ChainAudit<'a> {
    pub position: u64,
    pub at: Timestamp,
    pub client: Option<ClientId>,
    pub actor: Option<&'a str>,
    pub action: &'a str,
    pub target: Option<&'a str>,
    pub request_id: Option<&'a str>,
    pub outcome: &'a str,
    /// The event's details (before and after, status, code), as stored.
    pub details: &'a str,
}

/// The hash of an audit event's link.
pub fn audit_hash(previous: &ChainHash, link: &ChainAudit<'_>) -> ChainHash {
    let mut out = Encoder::new(AUDIT);
    out.bytes(previous);
    out.u64(link.position);
    out.i64(link.at.as_micros());
    out.option(link.client, |o, c| o.bytes(c.as_uuid().as_bytes()));
    out.option(link.actor, |o, a| o.text(a));
    out.text(link.action);
    out.option(link.target, |o, t| o.text(t));
    out.option(link.request_id, |o, r| o.text(r));
    out.text(link.outcome);
    out.text(link.details);
    out.finish()
}

/// What a ledger receipt signs: that this entry is at this position of the
/// chain, with this hash (INV-26).
pub fn receipt_message(position: u64, entry: EntryId, hash: &ChainHash) -> Vec<u8> {
    let mut out = RECEIPT.to_vec();
    out.extend_from_slice(&position.to_be_bytes());
    out.extend_from_slice(entry.as_uuid().as_bytes());
    out.extend_from_slice(hash);
    out
}

/// What a checkpoint signs: the chain's head at a period's close.
pub fn checkpoint_message(
    id: i64,
    period: i64,
    position: u64,
    hash: &ChainHash,
    audit_position: u64,
    audit_hash: &ChainHash,
    at: Timestamp,
) -> Vec<u8> {
    let mut out = CHECKPOINT.to_vec();
    for value in [id, period] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.extend_from_slice(&position.to_be_bytes());
    out.extend_from_slice(hash);
    out.extend_from_slice(&audit_position.to_be_bytes());
    out.extend_from_slice(audit_hash);
    out.extend_from_slice(&at.as_micros().to_be_bytes());
    out
}

/// Hex, lower case, as hashes and signatures are shown and stored as text.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::{ActorId, Authorization, EntryKind, ReasonCode};
    use crate::currency::Currency;
    use crate::model::PostedLine;
    use crate::money::{Money, PositiveAmount};

    fn id<T: std::str::FromStr>(n: u64) -> T
    where
        T::Err: std::fmt::Debug,
    {
        format!("00000000-0000-7000-8000-{n:012x}")
            .parse()
            .unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn entry(amount: i64, memo: &str) -> PostedEntry {
        entry_by(amount, memo, None)
    }

    fn entry_by(amount: i64, memo: &str, authorization: Option<Authorization>) -> PostedEntry {
        let amount = PositiveAmount::new(Money::from_minor(amount, Currency::ETB))
            .unwrap_or_else(|e| panic!("{e:?}"));
        PostedEntry::new(
            id(1),
            7,
            vec![
                PostedLine::new(id(10), Side::Debit, amount, amount.amount(), 0),
                PostedLine::new(id(11), Side::Credit, amount, 0, amount.amount()),
            ],
            None,
            BTreeMap::from([("memo".to_owned(), memo.to_owned())]),
            None,
            if authorization.is_some() {
                EntryKind::Reversal
            } else {
                EntryKind::Standard
            },
            authorization,
        )
    }

    fn link<'a>(entry: &'a PostedEntry) -> ChainEntry<'a> {
        ChainEntry {
            position: 1,
            entry,
            client: id(99),
            idempotency_key: "k-1",
            recorded_at: Timestamp::from_micros(1_000),
            period: Some(1),
        }
    }

    #[test]
    fn every_field_changes_the_hash() {
        let base = entry(500, "a");
        let h = entry_hash(&GENESIS, &link(&base));
        assert_eq!(h, entry_hash(&GENESIS, &link(&base)), "deterministic");
        let other_amount = entry(501, "a");
        let other_memo = entry(500, "b");
        let mut variants = vec![
            entry_hash(&[1; 32], &link(&base)),
            entry_hash(&GENESIS, &link(&other_amount)),
            entry_hash(&GENESIS, &link(&other_memo)),
        ];
        let mut moved = link(&base);
        moved.position = 2;
        variants.push(entry_hash(&GENESIS, &moved));
        let mut later = link(&base);
        later.recorded_at = Timestamp::from_micros(1_001);
        variants.push(entry_hash(&GENESIS, &later));
        let mut rekeyed = link(&base);
        rekeyed.idempotency_key = "k-2";
        variants.push(entry_hash(&GENESIS, &rekeyed));
        let mut no_period = link(&base);
        no_period.period = None;
        variants.push(entry_hash(&GENESIS, &no_period));
        let authorized = entry_by(
            500,
            "a",
            Some(Authorization {
                reason: ReasonCode::new("booking_error").unwrap_or_else(|e| panic!("{e:?}")),
                requested_by: ActorId::new("ops:a").unwrap_or_else(|e| panic!("{e:?}")),
                approved_by: None,
            }),
        );
        variants.push(entry_hash(&GENESIS, &link(&authorized)));
        for v in &variants {
            assert_ne!(*v, h);
        }
    }

    #[test]
    fn audit_hashes_cover_every_field() {
        let event = ChainAudit {
            position: 1,
            at: Timestamp::from_micros(5),
            client: Some(id(2)),
            actor: Some("app:wallet"),
            action: "operations.post",
            target: Some("entry:1"),
            request_id: Some("r"),
            outcome: "ok",
            details: "{}",
        };
        let h = audit_hash(&GENESIS, &event);
        let mut changed = event;
        changed.outcome = "refused";
        assert_ne!(audit_hash(&GENESIS, &changed), h);
        let mut no_actor = event;
        no_actor.actor = None;
        assert_ne!(audit_hash(&GENESIS, &no_actor), h);
    }

    #[test]
    fn messages_are_tagged_and_hex_is_lower_case() {
        let hash = [0xab; 32];
        let receipt = receipt_message(3, id(1), &hash);
        assert!(receipt.starts_with(b"mezgeb.receipt.v1\0"));
        let checkpoint = checkpoint_message(1, 1, 3, &hash, 2, &hash, Timestamp::from_micros(9));
        assert!(checkpoint.starts_with(b"mezgeb.checkpoint.v1\0"));
        assert_ne!(receipt, checkpoint);
        assert_eq!(hex(&[0x00, 0x0f, 0xab]), "000fab");
    }
}

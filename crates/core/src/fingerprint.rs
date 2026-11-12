//! Request fingerprints for idempotency (ADR-0008, step 1.11).
//!
//! A fingerprint is the SHA-256 of a canonical, versioned byte encoding of
//! what a client asked for. Two requests with the same idempotency key must
//! have the same fingerprint, or the second is refused: a key cannot be
//! reused for a different request.
//!
//! The entry ID is not part of the fingerprint: the ledger assigns it, so a
//! retry of the same request may carry a different one.

use sha2::{Digest, Sha256};

use crate::chart::Side;
use crate::entry::EntryDraft;
use crate::ids::EntryId;

/// A SHA-256 fingerprint of a request.
pub type Fingerprint = [u8; 32];

/// Bumped if the encoding ever changes, so old and new fingerprints can
/// never collide.
const VERSION: &[u8] = b"mezgeb.request.v1\0";

const POST: u8 = 1;
const REVERSE: u8 = 2;

fn length(hasher: &mut Sha256, n: usize) {
    // Lengths are bounded by the posting limits, far below u32::MAX.
    hasher.update(u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes());
}

fn text(hasher: &mut Sha256, value: &str) {
    length(hasher, value.len());
    hasher.update(value.as_bytes());
}

impl EntryDraft {
    /// The fingerprint of a request to post this draft. Every field the
    /// client controls is included, in order: the lines (account, side,
    /// currency, amount), the metadata (sorted by key), the event time and
    /// whether duplicate lines are allowed.
    pub fn fingerprint(&self) -> Fingerprint {
        let mut hasher = Sha256::new();
        hasher.update(VERSION);
        hasher.update([POST]);
        hasher.update([u8::from(self.allow_duplicate_lines)]);
        match self.event_time {
            Some(time) => {
                hasher.update([1]);
                hasher.update(time.as_micros().to_be_bytes());
            }
            None => hasher.update([0]),
        }
        length(&mut hasher, self.metadata.len());
        for (key, value) in &self.metadata {
            text(&mut hasher, key);
            text(&mut hasher, value);
        }
        length(&mut hasher, self.lines.len());
        for line in &self.lines {
            hasher.update(line.account.as_uuid().as_bytes());
            hasher.update([match line.side {
                Side::Debit => 1,
                Side::Credit => 2,
            }]);
            hasher.update(line.amount.currency().code().as_bytes());
            hasher.update(line.amount.amount().to_be_bytes());
        }
        hasher.finalize().into()
    }
}

/// The fingerprint of a request to reverse `original`.
pub fn reversal_fingerprint(original: EntryId) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(VERSION);
    hasher.update([REVERSE]);
    hasher.update(original.as_uuid().as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::currency::Currency;
    use crate::money::Money;
    use crate::time::Timestamp;
    use crate::validate::test_support::account_id;

    fn etb(amount: i64) -> Money {
        Money::from_minor(amount, Currency::ETB)
    }

    fn draft() -> EntryDraft {
        let mut draft = EntryDraft::default()
            .debit(account_id(1), etb(1_000))
            .credit(account_id(2), etb(1_000));
        draft.metadata.insert("payment".into(), "qr-1".into());
        draft
    }

    #[test]
    fn the_same_request_has_the_same_fingerprint() {
        assert_eq!(draft().fingerprint(), draft().fingerprint());
        // Metadata is a sorted map: insertion order does not matter.
        let mut a = draft();
        a.metadata.insert("b".into(), "2".into());
        a.metadata.insert("a".into(), "1".into());
        let mut b = draft();
        b.metadata.insert("a".into(), "1".into());
        b.metadata.insert("b".into(), "2".into());
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn every_field_changes_the_fingerprint() {
        let base = draft().fingerprint();
        let mut changes: Vec<EntryDraft> = Vec::new();

        let mut amount = draft();
        amount.lines[0].amount = etb(1_001);
        changes.push(amount);

        let mut currency = draft();
        currency.lines[0].amount = Money::from_minor(1_000, Currency::USD);
        changes.push(currency);

        let mut account = draft();
        account.lines[0].account = account_id(3);
        changes.push(account);

        let mut side = draft();
        side.lines[0].side = Side::Credit;
        changes.push(side);

        let mut order = draft();
        order.lines.reverse();
        changes.push(order);

        let mut metadata = draft();
        metadata.metadata.insert("payment".into(), "qr-2".into());
        changes.push(metadata);

        let mut event_time = draft();
        event_time.event_time = Some(Timestamp::from_micros(1));
        changes.push(event_time);

        let mut duplicates = draft();
        duplicates.allow_duplicate_lines = true;
        changes.push(duplicates);

        let mut extra_line = draft();
        extra_line.lines.push(extra_line.lines[0].clone());
        changes.push(extra_line);

        for (i, changed) in changes.iter().enumerate() {
            assert_ne!(changed.fingerprint(), base, "change {i}");
        }
    }

    #[test]
    fn metadata_boundaries_are_unambiguous() {
        // "ab" + "c" must differ from "a" + "bc".
        let mut a = EntryDraft::default();
        a.metadata.insert("ab".into(), "c".into());
        let mut b = EntryDraft::default();
        b.metadata.insert("a".into(), "bc".into());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn reversals_and_posts_never_share_a_fingerprint() -> Result<(), crate::ids::IdError> {
        let original: EntryId = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f".parse()?;
        let other: EntryId = "017f22e2-79b0-7cc3-98c4-dc0c0c073990".parse()?;
        assert_eq!(
            reversal_fingerprint(original),
            reversal_fingerprint(original)
        );
        assert_ne!(reversal_fingerprint(original), reversal_fingerprint(other));
        assert_ne!(
            reversal_fingerprint(original),
            EntryDraft::default().fingerprint()
        );
        Ok(())
    }
}

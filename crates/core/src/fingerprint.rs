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

use crate::authorization::Authorization;
use crate::chart::Side;
use crate::entry::EntryDraft;
use crate::ids::{AccountId, EntryId, HoldId};
use crate::money::PositiveAmount;
use crate::time::Timestamp;

/// A SHA-256 fingerprint of a request.
pub type Fingerprint = [u8; 32];

/// Bumped if the encoding ever changes, so old and new fingerprints can
/// never collide.
const VERSION: &[u8] = b"mezgeb.request.v1\0";

const POST: u8 = 1;
const REVERSE: u8 = 2;
const ADJUSTMENT: u8 = 3;
const HOLD_PLACE: u8 = 4;
const HOLD_CAPTURE: u8 = 5;
const HOLD_RELEASE: u8 = 6;
const HOLD_EXPIRE: u8 = 7;

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

fn authorization(hasher: &mut Sha256, auth: &Authorization) {
    text(hasher, auth.reason.as_str());
    text(hasher, auth.requested_by.as_str());
    match &auth.approved_by {
        Some(approver) => {
            hasher.update([1]);
            text(hasher, approver.as_str());
        }
        None => hasher.update([0]),
    }
}

/// The fingerprint of a request to reverse `original`, with its reason and
/// requester.
pub fn reversal_fingerprint(original: EntryId, auth: &Authorization) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(VERSION);
    hasher.update([REVERSE]);
    hasher.update(original.as_uuid().as_bytes());
    authorization(&mut hasher, auth);
    hasher.finalize().into()
}

/// The fingerprint of a request to post `draft` as an adjustment.
pub fn adjustment_fingerprint(draft: &EntryDraft, auth: &Authorization) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(VERSION);
    hasher.update([ADJUSTMENT]);
    hasher.update(draft.fingerprint());
    authorization(&mut hasher, auth);
    hasher.finalize().into()
}

fn amount(hasher: &mut Sha256, value: PositiveAmount) {
    hasher.update(value.currency().code().as_bytes());
    hasher.update(value.amount().to_be_bytes());
}

/// The fingerprint of a request to place hold `hold`.
pub fn hold_place_fingerprint(
    hold: HoldId,
    account: AccountId,
    held: AccountId,
    value: PositiveAmount,
    expires_at: Timestamp,
) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(VERSION);
    hasher.update([HOLD_PLACE]);
    hasher.update(hold.as_uuid().as_bytes());
    hasher.update(account.as_uuid().as_bytes());
    hasher.update(held.as_uuid().as_bytes());
    amount(&mut hasher, value);
    hasher.update(expires_at.as_micros().to_be_bytes());
    hasher.finalize().into()
}

/// The fingerprint of a request to capture `value` of hold `hold`.
pub fn hold_capture_fingerprint(
    hold: HoldId,
    destination: AccountId,
    value: PositiveAmount,
) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(VERSION);
    hasher.update([HOLD_CAPTURE]);
    hasher.update(hold.as_uuid().as_bytes());
    hasher.update(destination.as_uuid().as_bytes());
    amount(&mut hasher, value);
    hasher.finalize().into()
}

/// The fingerprint of a request to release (or, if `expire`, to expire)
/// hold `hold`.
pub fn hold_end_fingerprint(hold: HoldId, expire: bool) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(VERSION);
    hasher.update([if expire { HOLD_EXPIRE } else { HOLD_RELEASE }]);
    hasher.update(hold.as_uuid().as_bytes());
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

    fn auth(reason: &str, maker: &str, checker: Option<&str>) -> Authorization {
        Authorization {
            reason: crate::authorization::ReasonCode::new(reason)
                .unwrap_or_else(|_| unreachable!("valid test reason")),
            requested_by: crate::authorization::ActorId::new(maker)
                .unwrap_or_else(|_| unreachable!("valid test actor")),
            approved_by: checker.map(|c| {
                crate::authorization::ActorId::new(c)
                    .unwrap_or_else(|_| unreachable!("valid test actor"))
            }),
        }
    }

    #[test]
    fn reversals_and_posts_never_share_a_fingerprint() -> Result<(), crate::ids::IdError> {
        let original: EntryId = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f".parse()?;
        let other: EntryId = "017f22e2-79b0-7cc3-98c4-dc0c0c073990".parse()?;
        let why = auth("booking_error", "ops:1", None);
        assert_eq!(
            reversal_fingerprint(original, &why),
            reversal_fingerprint(original, &why)
        );
        assert_ne!(
            reversal_fingerprint(original, &why),
            reversal_fingerprint(other, &why)
        );
        assert_ne!(
            reversal_fingerprint(original, &why),
            reversal_fingerprint(original, &auth("duplicate_posting", "ops:1", None))
        );
        assert_ne!(
            reversal_fingerprint(original, &why),
            EntryDraft::default().fingerprint()
        );
        Ok(())
    }

    #[test]
    fn adjustments_include_who_and_why() {
        let base = adjustment_fingerprint(&draft(), &auth("goodwill_credit", "a:1", Some("b:2")));
        assert_ne!(base, draft().fingerprint());
        for other in [
            auth("compensation", "a:1", Some("b:2")),
            auth("goodwill_credit", "a:9", Some("b:2")),
            auth("goodwill_credit", "a:1", Some("b:9")),
            auth("goodwill_credit", "a:1", None),
        ] {
            assert_ne!(adjustment_fingerprint(&draft(), &other), base);
        }
    }

    #[test]
    fn hold_requests_have_distinct_fingerprints() -> Result<(), Box<dyn std::error::Error>> {
        let hold: HoldId = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f".parse()?;
        let other: HoldId = "017f22e2-79b0-7cc3-98c4-dc0c0c073990".parse()?;
        let value = PositiveAmount::new(etb(100))?;
        let more = PositiveAmount::new(etb(101))?;
        let at = Timestamp::from_micros(5);
        let place = hold_place_fingerprint(hold, account_id(1), account_id(2), value, at);
        let all = [
            place,
            hold_place_fingerprint(other, account_id(1), account_id(2), value, at),
            hold_place_fingerprint(hold, account_id(3), account_id(2), value, at),
            hold_place_fingerprint(hold, account_id(1), account_id(3), value, at),
            hold_place_fingerprint(hold, account_id(1), account_id(2), more, at),
            hold_place_fingerprint(
                hold,
                account_id(1),
                account_id(2),
                value,
                Timestamp::from_micros(6),
            ),
            hold_capture_fingerprint(hold, account_id(2), value),
            hold_capture_fingerprint(hold, account_id(3), value),
            hold_capture_fingerprint(hold, account_id(2), more),
            hold_end_fingerprint(hold, false),
            hold_end_fingerprint(hold, true),
            hold_end_fingerprint(other, false),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(
            place,
            hold_place_fingerprint(hold, account_id(1), account_id(2), value, at)
        );
        Ok(())
    }
}

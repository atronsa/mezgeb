//! Refunds: money given back against an earlier entry (step 2.04, INV-15,
//! ADR-0015).
//!
//! A refund is a new entry that names the entry it refunds. It does not have
//! to mirror it: a partial refund gives back part of a payment, and the money
//! may come from somewhere the original never touched (a dispute's held
//! funds, for example). What it may never do is give back more than was paid,
//! or give it to someone else. For every account of the original, by its net
//! effect in the original:
//!
//! - an account the original **debited** (the payer) may only be credited,
//!   and by at most what the original debited it;
//! - an account the original **credited** (the merchant, fee revenue) may
//!   only be debited, and by at most what the original credited it;
//! - an account the original did not touch may only be debited: money can
//!   come from anywhere, but goes back only to whoever paid.
//!
//! The limits apply to all of an entry's refunds together, so a second
//! refund can only give back what the first left. Only a standard entry can
//! be refunded, never once it is reversed, and an entry with refunds can no
//! longer be reversed: either would give the money back twice.
//!
//! The rule is pure. The reference ledger and the engine both use it; the
//! engine applies it with the original's refund tallies locked, so concurrent
//! refunds of one entry queue and the limits hold (`refund_tallies`,
//! migration 0006).

use std::collections::BTreeMap;

use crate::chart::Side;
use crate::entry::LineDraft;
use crate::ids::AccountId;
use crate::model::{LedgerError, PostedEntry, PostedLine};

/// What the refunds of one entry may move back, per account of the
/// original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundLimits {
    /// For each account the original moved: the side a refund line on it
    /// must have, and the most all refunds together may move.
    limits: BTreeMap<AccountId, (Side, i64)>,
}

impl RefundLimits {
    /// The limits of `original`, from its net effect on each account. An
    /// account the original debited and credited by the same amount has no
    /// limit, and so behaves like an account it did not touch.
    pub fn of(original: &PostedEntry) -> RefundLimits {
        RefundLimits::from_lines(original.lines())
    }

    /// The limits of an entry with these lines.
    pub fn from_lines(lines: &[PostedLine]) -> RefundLimits {
        let mut net: BTreeMap<AccountId, i128> = BTreeMap::new();
        for line in lines {
            let amount = i128::from(line.amount().amount());
            *net.entry(line.account()).or_insert(0) += match line.side() {
                Side::Debit => amount,
                Side::Credit => -amount,
            };
        }
        let limits = net
            .into_iter()
            .filter_map(|(account, net)| {
                // A posted entry's amounts fit in an i64, so its net does too.
                let limit = i64::try_from(net.unsigned_abs()).ok()?;
                match net {
                    n if n > 0 => Some((account, (Side::Credit, limit))),
                    n if n < 0 => Some((account, (Side::Debit, limit))),
                    _ => None,
                }
            })
            .collect();
        RefundLimits { limits }
    }

    /// Each account with a limit: the side a refund must use, and the most
    /// all refunds together may move.
    pub fn accounts(&self) -> impl Iterator<Item = (AccountId, Side, i64)> + '_ {
        self.limits
            .iter()
            .map(|(account, (side, limit))| (*account, *side, *limit))
    }

    /// Checks one refund's lines against the limits, given what earlier
    /// refunds of the same entry already moved (`refunded`, per account).
    /// Returns the new totals for the accounts this refund moves.
    ///
    /// Lines whose amount is not positive are left to the posting rules,
    /// which refuse them.
    pub fn check(
        &self,
        lines: &[LineDraft],
        refunded: &BTreeMap<AccountId, i64>,
    ) -> Result<BTreeMap<AccountId, i64>, LedgerError> {
        let mut moved: BTreeMap<AccountId, (i128, usize)> = BTreeMap::new();
        for (line, draft) in lines.iter().enumerate() {
            let amount = draft.amount.amount();
            if amount <= 0 {
                continue;
            }
            match self.limits.get(&draft.account) {
                Some((side, _)) if *side == draft.side => {
                    let entry = moved.entry(draft.account).or_insert((0, line));
                    entry.0 += i128::from(amount);
                }
                Some(_) => return Err(LedgerError::RefundLineNotAllowed { line }),
                None if draft.side == Side::Debit => {}
                None => return Err(LedgerError::RefundLineNotAllowed { line }),
            }
        }
        // In line order, so the first line over its limit is the one
        // reported, whatever the account IDs.
        let mut moved: Vec<(AccountId, (i128, usize))> = moved.into_iter().collect();
        moved.sort_by_key(|(_, (_, line))| *line);
        let mut totals = BTreeMap::new();
        for (account, (amount, line)) in moved {
            let limit = self.limits.get(&account).map_or(0, |(_, limit)| *limit);
            let before = refunded.get(&account).copied().unwrap_or(0);
            let total = i128::from(before) + amount;
            if total > i128::from(limit) {
                return Err(LedgerError::RefundExceedsOriginal { line });
            }
            // At most `limit`, so it fits in an i64.
            totals.insert(account, i64::try_from(total).unwrap_or(limit));
        }
        Ok(totals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::EntryKind;
    use crate::currency::Currency;
    use crate::ids::EntryId;
    use crate::money::{Money, PositiveAmount};
    use crate::validate::test_support::account_id;

    fn etb(amount: i64) -> Money {
        Money::from_minor(amount, Currency::ETB)
    }

    fn line(account: u64, side: Side, amount: i64) -> Result<PostedLine, crate::money::MoneyError> {
        Ok(PostedLine::new(
            account_id(account),
            side,
            PositiveAmount::new(etb(amount))?,
            0,
            0,
        ))
    }

    /// E07: Abebe (0) pays 100, the merchant (1) gets 98, fee revenue (2) 2.
    fn payment() -> Result<PostedEntry, Box<dyn std::error::Error>> {
        let id: EntryId = "00000000-0000-7000-8000-000000000007".parse()?;
        Ok(PostedEntry::new(
            id,
            1,
            vec![
                line(0, Side::Debit, 10_000)?,
                line(1, Side::Credit, 9_800)?,
                line(2, Side::Credit, 200)?,
            ],
            None,
            BTreeMap::new(),
            None,
            EntryKind::Standard,
            None,
        ))
    }

    fn draft(account: u64, side: Side, amount: i64) -> LineDraft {
        LineDraft {
            account: account_id(account),
            side,
            amount: etb(amount),
        }
    }

    #[test]
    fn limits_follow_the_original() -> Result<(), Box<dyn std::error::Error>> {
        let limits = RefundLimits::of(&payment()?);
        let all: Vec<_> = limits.accounts().collect();
        assert_eq!(
            all,
            [
                (account_id(0), Side::Credit, 10_000),
                (account_id(1), Side::Debit, 9_800),
                (account_id(2), Side::Debit, 200),
            ]
        );
        Ok(())
    }

    #[test]
    fn partial_refunds_add_up_to_the_original() -> Result<(), Box<dyn std::error::Error>> {
        let limits = RefundLimits::of(&payment()?);
        // E12: 40 back, 0.80 of it from the fee.
        let e12 = [
            draft(1, Side::Debit, 3_920),
            draft(2, Side::Debit, 80),
            draft(0, Side::Credit, 4_000),
        ];
        let after = limits.check(&e12, &BTreeMap::new())?;
        assert_eq!(after[&account_id(0)], 4_000);
        // The rest, exactly.
        let rest = [
            draft(1, Side::Debit, 5_880),
            draft(2, Side::Debit, 120),
            draft(0, Side::Credit, 6_000),
        ];
        let full = limits.check(&rest, &after)?;
        assert_eq!(
            full,
            BTreeMap::from([
                (account_id(0), 10_000),
                (account_id(1), 9_800),
                (account_id(2), 200)
            ])
        );
        // One santim more, on any account, is refused.
        let more = [draft(1, Side::Debit, 1), draft(0, Side::Credit, 1)];
        assert_eq!(
            limits.check(&more, &full),
            Err(LedgerError::RefundExceedsOriginal { line: 0 })
        );
        Ok(())
    }

    #[test]
    fn money_goes_back_only_to_whoever_paid() -> Result<(), Box<dyn std::error::Error>> {
        let limits = RefundLimits::of(&payment()?);
        let none = BTreeMap::new();
        // Crediting someone the original never debited (account 9).
        let elsewhere = [draft(1, Side::Debit, 100), draft(9, Side::Credit, 100)];
        assert_eq!(
            limits.check(&elsewhere, &none),
            Err(LedgerError::RefundLineNotAllowed { line: 1 })
        );
        // Crediting the merchant, or debiting the payer: the wrong way.
        let wrong_way = [draft(0, Side::Debit, 100), draft(1, Side::Credit, 100)];
        assert_eq!(
            limits.check(&wrong_way, &none),
            Err(LedgerError::RefundLineNotAllowed { line: 0 })
        );
        // Money may come from an account the original never touched, such
        // as a dispute's held funds (9).
        let from_dispute = [draft(9, Side::Debit, 9_800), draft(0, Side::Credit, 9_800)];
        let after = limits.check(&from_dispute, &none)?;
        assert_eq!(after, BTreeMap::from([(account_id(0), 9_800)]));
        // The payer's limit still counts it.
        let then_refund = [draft(1, Side::Debit, 201), draft(0, Side::Credit, 201)];
        assert_eq!(
            limits.check(&then_refund, &after),
            Err(LedgerError::RefundExceedsOriginal { line: 1 })
        );
        Ok(())
    }
}

//! Posting rules: turns an [`EntryDraft`] into a [`BalancedEntry`] or says
//! exactly why not. Pure: no I/O, no clock, the same answer every time.

use std::collections::{BTreeMap, HashSet};

use crate::currency::Currency;
use crate::entry::{
    AccountsView, BalancedEntry, EntryDraft, MAX_LINES, MAX_METADATA_BYTES, MAX_METADATA_KEYS,
};
use crate::error::PostingError;
use crate::money::PositiveAmount;

/// Checks every posting rule, in the order listed in [`crate::error`], and
/// returns the first one that fails.
///
/// This is the only way to create a [`BalancedEntry`].
pub fn validate<V: AccountsView + ?Sized>(
    draft: EntryDraft,
    accounts: &V,
) -> Result<BalancedEntry, PostingError> {
    if draft.lines.len() < 2 {
        return Err(PostingError::TooFewLines);
    }
    if draft.lines.len() > MAX_LINES {
        return Err(PostingError::TooManyLines);
    }
    let metadata_bytes: usize = draft.metadata.iter().map(|(k, v)| k.len() + v.len()).sum();
    if draft.metadata.len() > MAX_METADATA_KEYS || metadata_bytes > MAX_METADATA_BYTES {
        return Err(PostingError::MetadataTooLarge);
    }

    let mut lines = Vec::with_capacity(draft.lines.len());
    for (line, requested) in draft.lines.iter().enumerate() {
        let amount = PositiveAmount::new(requested.amount)
            .map_err(|_| PostingError::AmountNotPositive { line })?;
        let account = accounts
            .account(requested.account)
            .ok_or(PostingError::AccountNotFound { line })?;
        if !account.status().accepts_postings() {
            return Err(PostingError::AccountNotOpen {
                line,
                status: account.status(),
            });
        }
        if account.currency() != amount.currency() {
            return Err(PostingError::CurrencyMismatch {
                line,
                expected: account.currency(),
                found: amount.currency(),
            });
        }
        lines.push(BalancedEntry::line(
            requested.account,
            requested.side,
            amount,
        ));
    }

    if !draft.allow_duplicate_lines {
        let mut seen = HashSet::new();
        for (line, l) in lines.iter().enumerate() {
            if !seen.insert((l.account(), l.side())) {
                return Err(PostingError::DuplicateLine { line });
            }
        }
    }

    // Sum in i128: at most MAX_LINES amounts of at most i64::MAX each, so
    // the sums cannot overflow.
    let mut sums: BTreeMap<Currency, (i128, i128)> = BTreeMap::new();
    for l in &lines {
        let amount = l.amount();
        let (debits, credits) = sums.entry(amount.currency()).or_insert((0, 0));
        match l.side() {
            crate::chart::Side::Debit => *debits += i128::from(amount.amount()),
            crate::chart::Side::Credit => *credits += i128::from(amount.amount()),
        }
    }
    let mut totals = Vec::with_capacity(sums.len());
    for (currency, (debits, credits)) in sums {
        let overflow = PostingError::AmountOverflow { currency };
        let debits = i64::try_from(debits).map_err(|_| overflow)?;
        let credits = i64::try_from(credits).map_err(|_| overflow)?;
        totals.push((currency, debits, credits));
    }
    if let Some(&(currency, debits, credits)) = totals.iter().find(|(_, d, c)| d != c) {
        return Err(PostingError::Unbalanced {
            currency,
            debits,
            credits,
        });
    }

    Ok(BalancedEntry::new(lines, draft.metadata, draft.event_time))
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;

    use crate::account::Account;
    use crate::chart::Chart;
    use crate::currency::Currency;
    use crate::ids::AccountId;

    /// A UUIDv7 account ID built from a number, for tests.
    pub fn account_id(n: u64) -> AccountId {
        let mut bytes = [0u8; 16];
        bytes[8..].copy_from_slice(&n.to_be_bytes());
        bytes[6] = 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        AccountId::from_uuid(uuid::Uuid::from_bytes(bytes))
            .unwrap_or_else(|_| unreachable!("version and variant bits are set"))
    }

    fn chart(currency: Currency) -> Option<Chart> {
        Chart::from_toml(&format!(
            "name = \"T\"\ncurrency = \"{currency}\"\n[[accounts]]\ncode = 2010\nname = \"Wallets\"\ntype = \"liability\"\nkind = \"control\"\nprefix = \"cust\"\nstatus = \"active\"\n"
        ))
        .ok()
    }

    /// `n` open wallet accounts per currency, with IDs `account_id(0..)`.
    pub fn accounts(currencies: &[Currency]) -> HashMap<AccountId, Account> {
        let mut map = HashMap::new();
        for (i, currency) in currencies.iter().enumerate() {
            let Some(chart) = chart(*currency) else {
                continue;
            };
            let id = account_id(i as u64);
            if let Ok(account) = Account::open(id, &chart, &format!("2010:cust-{i}")) {
                map.insert(id, account);
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{account_id, accounts};
    use super::*;
    use crate::account::AccountStatus;
    use crate::chart::Side;
    use crate::entry::LineDraft;
    use crate::money::Money;
    use crate::time::Timestamp;

    const ETB: Currency = Currency::ETB;

    fn etb(amount: i64) -> Money {
        Money::from_minor(amount, ETB)
    }

    fn transfer(amount: i64) -> EntryDraft {
        EntryDraft::default()
            .debit(account_id(0), etb(amount))
            .credit(account_id(1), etb(amount))
    }

    #[test]
    fn accepts_a_balanced_entry() -> Result<(), PostingError> {
        let view = accounts(&[ETB, ETB, ETB]);
        let mut draft = EntryDraft::default()
            .debit(account_id(0), etb(10_000))
            .credit(account_id(1), etb(9_800))
            .credit(account_id(2), etb(200));
        draft.metadata.insert("payment".into(), "qr-1".into());
        draft.event_time = Some(Timestamp::from_micros(42));

        let entry = validate(draft, &view)?;
        assert_eq!(entry.lines().len(), 3);
        assert_eq!(entry.lines()[0].account(), account_id(0));
        assert_eq!(entry.lines()[0].side(), Side::Debit);
        assert_eq!(entry.lines()[2].amount().amount(), 200);
        assert_eq!(
            entry.metadata().get("payment").map(String::as_str),
            Some("qr-1")
        );
        assert_eq!(entry.event_time(), Some(Timestamp::from_micros(42)));
        assert_eq!(entry.totals(), BTreeMap::from([(ETB, 10_000)]));
        Ok(())
    }

    #[test]
    fn balances_each_currency_separately() -> Result<(), PostingError> {
        let usd = Currency::USD;
        let view = accounts(&[ETB, ETB, usd, usd]);
        let draft = EntryDraft::default()
            .debit(account_id(0), etb(500))
            .credit(account_id(1), etb(500))
            .debit(account_id(2), Money::from_minor(7, usd))
            .credit(account_id(3), Money::from_minor(7, usd));
        let entry = validate(draft, &view)?;
        assert_eq!(entry.totals(), BTreeMap::from([(ETB, 500), (usd, 7)]));

        // Balanced in total, but not per currency.
        let mixed = EntryDraft::default()
            .debit(account_id(0), etb(5))
            .credit(account_id(2), Money::from_minor(5, usd));
        assert_eq!(
            validate(mixed, &view),
            Err(PostingError::Unbalanced {
                currency: ETB,
                debits: 5,
                credits: 0
            })
        );
        Ok(())
    }

    #[test]
    fn rule_too_few_lines() {
        let view = accounts(&[ETB]);
        assert_eq!(
            validate(EntryDraft::default(), &view),
            Err(PostingError::TooFewLines)
        );
        let one = EntryDraft::default().debit(account_id(0), etb(1));
        assert_eq!(validate(one, &view), Err(PostingError::TooFewLines));
    }

    #[test]
    fn rule_too_many_lines() {
        let view = accounts(&[ETB, ETB]);
        let mut draft = transfer(1);
        draft.allow_duplicate_lines = true;
        draft.lines = std::iter::repeat_n(draft.lines.clone(), 51)
            .flatten()
            .collect();
        assert_eq!(validate(draft, &view), Err(PostingError::TooManyLines));
    }

    #[test]
    fn rule_metadata_too_large() {
        let view = accounts(&[ETB, ETB]);
        let mut many_keys = transfer(1);
        for i in 0..=MAX_METADATA_KEYS {
            many_keys.metadata.insert(format!("k{i}"), String::new());
        }
        assert_eq!(
            validate(many_keys, &view),
            Err(PostingError::MetadataTooLarge)
        );

        let mut big = transfer(1);
        big.metadata
            .insert("k".into(), "v".repeat(MAX_METADATA_BYTES));
        assert_eq!(validate(big, &view), Err(PostingError::MetadataTooLarge));

        let mut at_limit = transfer(1);
        at_limit
            .metadata
            .insert("k".into(), "v".repeat(MAX_METADATA_BYTES - 1));
        assert!(validate(at_limit, &view).is_ok());
    }

    #[test]
    fn rule_amount_not_positive() {
        let view = accounts(&[ETB, ETB]);
        for amount in [0, -1] {
            let draft = EntryDraft::default()
                .debit(account_id(0), etb(amount))
                .credit(account_id(1), etb(amount));
            assert_eq!(
                validate(draft, &view),
                Err(PostingError::AmountNotPositive { line: 0 })
            );
        }
    }

    #[test]
    fn rule_account_not_found() {
        let view = accounts(&[ETB]);
        assert_eq!(
            validate(transfer(1), &view),
            Err(PostingError::AccountNotFound { line: 1 })
        );
    }

    #[test]
    fn rule_account_not_open() -> Result<(), crate::account::StatusError> {
        for status in [AccountStatus::Frozen, AccountStatus::Closed] {
            let mut view = accounts(&[ETB, ETB]);
            if let Some(account) = view.get_mut(&account_id(1)) {
                account.set_status(status)?;
            }
            assert_eq!(
                validate(transfer(1), &view),
                Err(PostingError::AccountNotOpen { line: 1, status })
            );
        }
        Ok(())
    }

    #[test]
    fn rule_currency_mismatch() {
        let view = accounts(&[ETB, ETB]);
        let draft = EntryDraft::default()
            .debit(account_id(0), etb(1))
            .credit(account_id(1), Money::from_minor(1, Currency::USD));
        assert_eq!(
            validate(draft, &view),
            Err(PostingError::CurrencyMismatch {
                line: 1,
                expected: ETB,
                found: Currency::USD
            })
        );
    }

    #[test]
    fn rule_duplicate_line() {
        let view = accounts(&[ETB, ETB]);
        let draft = EntryDraft::default()
            .debit(account_id(0), etb(1))
            .debit(account_id(0), etb(1))
            .credit(account_id(1), etb(2));
        assert_eq!(
            validate(draft.clone(), &view),
            Err(PostingError::DuplicateLine { line: 1 })
        );
        let allowed = EntryDraft {
            allow_duplicate_lines: true,
            ..draft
        };
        assert!(validate(allowed, &view).is_ok());

        // The same account on opposite sides is not a duplicate.
        let both_sides = EntryDraft::default()
            .debit(account_id(0), etb(1))
            .credit(account_id(0), etb(1));
        assert!(validate(both_sides, &view).is_ok());
    }

    #[test]
    fn rule_amount_overflow() {
        let view = accounts(&[ETB, ETB, ETB]);
        let debit_overflow = EntryDraft::default()
            .debit(account_id(0), etb(i64::MAX))
            .debit(account_id(1), etb(1))
            .credit(account_id(2), etb(1));
        assert_eq!(
            validate(debit_overflow, &view),
            Err(PostingError::AmountOverflow { currency: ETB })
        );
        let credit_overflow = EntryDraft::default()
            .debit(account_id(0), etb(1))
            .credit(account_id(1), etb(i64::MAX))
            .credit(account_id(2), etb(1));
        assert_eq!(
            validate(credit_overflow, &view),
            Err(PostingError::AmountOverflow { currency: ETB })
        );
        // The largest single amount is fine.
        assert!(validate(transfer(i64::MAX), &view).is_ok());
    }

    #[test]
    fn rule_unbalanced() {
        let view = accounts(&[ETB, ETB]);
        let draft = EntryDraft::default()
            .debit(account_id(0), etb(100))
            .credit(account_id(1), etb(99));
        assert_eq!(
            validate(draft, &view),
            Err(PostingError::Unbalanced {
                currency: ETB,
                debits: 100,
                credits: 99
            })
        );
    }

    #[test]
    fn rules_are_checked_in_order() {
        let view = accounts(&[ETB]);
        // Both a non-positive amount (line 0) and a missing account (line 1):
        // the earlier line's error wins.
        let draft = EntryDraft {
            lines: vec![
                LineDraft {
                    account: account_id(0),
                    side: Side::Debit,
                    amount: etb(0),
                },
                LineDraft {
                    account: account_id(9),
                    side: Side::Credit,
                    amount: etb(1),
                },
            ],
            ..EntryDraft::default()
        };
        assert_eq!(
            validate(draft, &view),
            Err(PostingError::AmountNotPositive { line: 0 })
        );
    }
}

#[cfg(test)]
mod properties {
    use std::collections::HashMap;

    use proptest::prelude::*;

    use super::test_support::{account_id, accounts};
    use super::*;
    use crate::account::Account;
    use crate::chart::Side;
    use crate::ids::AccountId;
    use crate::money::Money;

    const CURRENCIES: [Currency; 2] = [Currency::ETB, Currency::USD];

    /// A balanced entry: `pairs` debit/credit pairs, each on its own two
    /// accounts and in one currency, lines shuffled. Returns the draft and
    /// the accounts it needs.
    fn balanced() -> impl Strategy<Value = (EntryDraft, HashMap<AccountId, Account>)> {
        proptest::collection::vec((1..=1_000_000_000i64, 0..CURRENCIES.len()), 1..10)
            .prop_flat_map(|pairs| {
                let mut currencies = Vec::new();
                let mut lines = Vec::new();
                for (i, (amount, c)) in pairs.iter().enumerate() {
                    let currency = CURRENCIES[*c];
                    currencies.extend([currency, currency]);
                    let money = Money::from_minor(*amount, currency);
                    lines.push((account_id(2 * i as u64), Side::Debit, money));
                    lines.push((account_id(2 * i as u64 + 1), Side::Credit, money));
                }
                (Just(currencies), Just(lines).prop_shuffle())
            })
            .prop_map(|(currencies, lines)| {
                let mut draft = EntryDraft::default();
                for (account, side, amount) in lines {
                    draft.lines.push(crate::entry::LineDraft {
                        account,
                        side,
                        amount,
                    });
                }
                (draft, accounts(&currencies))
            })
    }

    proptest! {
        #[test]
        fn generated_balanced_entries_are_accepted((draft, view) in balanced()) {
            prop_assert!(validate(draft, &view).is_ok());
        }

        /// Whatever validate accepts sums to zero in every currency.
        #[test]
        fn accepted_entries_balance_per_currency(
            lines in proptest::collection::vec(
                (0u64..4, any::<bool>(), 1..=5i64, 0..CURRENCIES.len()),
                0..8,
            ),
        ) {
            let view = accounts(&[Currency::ETB, Currency::ETB, Currency::USD, Currency::USD]);
            let mut draft = EntryDraft { allow_duplicate_lines: true, ..EntryDraft::default() };
            for (account, debit, amount, c) in lines {
                let money = Money::from_minor(amount, CURRENCIES[c]);
                draft = if debit { draft.debit(account_id(account), money) } else { draft.credit(account_id(account), money) };
            }
            if let Ok(entry) = validate(draft, &view) {
                let mut net: BTreeMap<Currency, i128> = BTreeMap::new();
                for line in entry.lines() {
                    let amount = i128::from(line.amount().amount());
                    let signed = if line.side() == Side::Debit { amount } else { -amount };
                    *net.entry(line.amount().currency()).or_insert(0) += signed;
                }
                prop_assert!(net.values().all(|n| *n == 0), "{:?}", net);
            }
        }

        /// Changing one amount of a balanced entry always unbalances it.
        #[test]
        fn changing_one_amount_is_rejected(
            (draft, view) in balanced(),
            pick in any::<proptest::sample::Index>(),
            delta in prop_oneof![-1_000i64..=-1, 1i64..=1_000],
        ) {
            let mut mutated = draft;
            let line = pick.index(mutated.lines.len());
            let original = mutated.lines[line].amount;
            let changed = original.amount() + delta;
            prop_assume!(changed > 0);
            mutated.lines[line].amount = Money::from_minor(changed, original.currency());
            let result = validate(mutated, &view);
            prop_assert!(
                matches!(result, Err(PostingError::Unbalanced { currency, .. }) if currency == original.currency()),
                "{:?}", result
            );
        }
    }
}

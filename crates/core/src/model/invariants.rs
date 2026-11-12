//! `check_invariants`: every invariant from `docs/invariants.md` that
//! applies to the ledger so far, checked from the stored data alone.
//!
//! Checked: INV-1, INV-2, INV-3, INV-5, INV-6, INV-7, INV-8, INV-12,
//! INV-14, INV-16, INV-17, INV-18, INV-24 (accounts exist), INV-25, and
//! the status rule that a closed account has a zero balance. Later steps add the rest (holds, periods,
//! idempotency, offline).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use crate::account::AccountStatus;
use crate::authorization::{Authorization, EntryKind};
use crate::chart::Side;
use crate::currency::Currency;
use crate::ids::AccountId;
use crate::model::ledger::MemLedger;

/// The first invariant found broken, with what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantViolation {
    /// The invariant ID, such as `"INV-5"`.
    pub invariant: &'static str,
    pub detail: String,
}

impl fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} violated: {}", self.invariant, self.detail)
    }
}

impl std::error::Error for InvariantViolation {}

fn fail(invariant: &'static str, detail: String) -> Result<(), InvariantViolation> {
    Err(InvariantViolation { invariant, detail })
}

/// Checks every applicable invariant and returns the first violation.
pub fn check_invariants(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    check_entries(ledger)?;
    check_running_totals(ledger)?;
    check_global_balance(ledger)?;
    check_balances(ledger)?;
    check_reversals(ledger)?;
    check_authorizations(ledger)?;
    check_holds(ledger)
}

/// INV-16 (a hold ends at most once, recorded by one closing entry), INV-17
/// (a capture never exceeds the hold) and INV-18 (a held account holds
/// exactly its active hold's amount, and nothing once the hold has ended).
pub(super) fn check_holds(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    use crate::hold::HoldStatus;
    for hold in ledger.holds() {
        let id = hold.id();
        let ended = hold.status() != HoldStatus::Active;
        if ended != hold.close_entry().is_some() {
            return fail(
                "INV-16",
                format!(
                    "hold {id} is {} but its closing entry is {:?}",
                    hold.status(),
                    hold.close_entry()
                ),
            );
        }
        if hold.captured() < 0
            || hold.captured() > hold.amount().amount()
            || (hold.captured() > 0) != (hold.status() == HoldStatus::Captured)
        {
            return fail(
                "INV-17",
                format!("hold {id} records a capture of {}", hold.captured()),
            );
        }
        let Some(held) = ledger.account(hold.held_account()) else {
            return fail("INV-18", format!("hold {id}'s held account is missing"));
        };
        if held.balance().amount() != hold.held_balance() {
            return fail(
                "INV-18",
                format!(
                    "hold {id} is {}: its held account holds {}, not {}",
                    hold.status(),
                    held.balance().amount(),
                    hold.held_balance()
                ),
            );
        }
    }
    Ok(())
}

/// INV-25 and INV-14: a reversal has a reversal reason and a requester; an
/// adjustment has an adjustment reason and a different requester and
/// approver; a standard entry has neither; only reversals reverse.
pub(super) fn check_authorizations(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    for entry in ledger.entries() {
        let id = entry.id();
        if (entry.kind() == EntryKind::Reversal) != entry.reverses().is_some() {
            return fail(
                "INV-14",
                format!("entry {id} is marked {} wrongly", entry.kind()),
            );
        }
        if let Err(error) = Authorization::check(entry.kind(), entry.authorization()) {
            return fail("INV-25", format!("entry {id}: {error}"));
        }
    }
    Ok(())
}

/// INV-1 (every entry has 2+ lines and balances per currency), INV-3 (line
/// currency is the account's), INV-12 (sequence strictly increasing) and
/// INV-24 (every line's account exists).
pub(super) fn check_entries(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    let mut previous = 0;
    for entry in ledger.entries() {
        let id = entry.id();
        if entry.sequence() <= previous {
            return fail(
                "INV-12",
                format!("entry {id} has sequence {}", entry.sequence()),
            );
        }
        previous = entry.sequence();
        if entry.lines().len() < 2 {
            return fail("INV-1", format!("entry {id} has fewer than 2 lines"));
        }
        let mut net: BTreeMap<Currency, i128> = BTreeMap::new();
        for line in entry.lines() {
            let Some(account) = ledger.account(line.account()) else {
                return fail("INV-24", format!("entry {id} posts to a missing account"));
            };
            let money = line.amount().money();
            if money.currency() != account.currency() {
                return fail(
                    "INV-3",
                    format!("entry {id} has a line in the wrong currency"),
                );
            }
            *net.entry(money.currency()).or_insert(0) += signed(line.side(), money.amount());
        }
        if let Some((currency, n)) = net.iter().find(|(_, n)| **n != 0) {
            return fail("INV-1", format!("entry {id} is off by {n} in {currency}"));
        }
    }
    Ok(())
}

/// INV-7 (replaying the journal from empty reproduces every balance), which
/// is checked as INV-6 (each line's stored totals equal the replay so far)
/// and INV-5 (each account's stored totals equal the full replay).
pub(super) fn check_running_totals(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    let mut replayed: HashMap<AccountId, (i64, i64)> = HashMap::new();
    for entry in ledger.entries() {
        for line in entry.lines() {
            let totals = replayed.entry(line.account()).or_insert((0, 0));
            let amount = line.amount().amount();
            let total = match line.side() {
                Side::Debit => &mut totals.0,
                Side::Credit => &mut totals.1,
            };
            let Some(next) = total.checked_add(amount) else {
                return fail("INV-7", format!("replaying entry {} overflows", entry.id()));
            };
            *total = next;
            if (line.debits_after(), line.credits_after()) != *totals {
                return fail(
                    "INV-6",
                    format!(
                        "entry {}: stored totals {:?} after a line, replay gives {:?}",
                        entry.id(),
                        (line.debits_after(), line.credits_after()),
                        totals
                    ),
                );
            }
        }
    }
    for account in ledger.accounts() {
        let stored = (account.debits_total(), account.credits_total());
        let replay = replayed.get(&account.id()).copied().unwrap_or((0, 0));
        if stored != replay {
            return fail(
                "INV-5",
                format!(
                    "account {} stores {stored:?}, its lines sum to {replay:?}",
                    account.reference()
                ),
            );
        }
    }
    Ok(())
}

/// INV-2: in every currency, all debits equal all credits.
pub(super) fn check_global_balance(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    let mut net: BTreeMap<Currency, i128> = BTreeMap::new();
    for account in ledger.accounts() {
        *net.entry(account.currency()).or_insert(0) +=
            i128::from(account.debits_total()) - i128::from(account.credits_total());
    }
    match net.iter().find(|(_, n)| **n != 0) {
        Some((currency, n)) => fail("INV-2", format!("{currency} debits exceed credits by {n}")),
        None => Ok(()),
    }
}

/// INV-8 (no account below zero unless allowed) and the rule that a closed
/// account has a zero balance.
pub(super) fn check_balances(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    for account in ledger.accounts() {
        let balance = account.balance();
        if balance.is_negative() && !account.allow_negative() {
            return fail(
                "INV-8",
                format!("account {} is at {balance}", account.reference()),
            );
        }
        if account.status() == AccountStatus::Closed && !balance.is_zero() {
            return fail(
                "INV-8",
                format!("closed account {} is at {balance}", account.reference()),
            );
        }
    }
    Ok(())
}

/// INV-14: an entry is reversed at most once, by its exact mirror, and a
/// reversal is never itself reversed.
pub(super) fn check_reversals(ledger: &MemLedger) -> Result<(), InvariantViolation> {
    let mut reversed = HashSet::new();
    for entry in ledger.entries() {
        let Some(original_id) = entry.reverses() else {
            continue;
        };
        let id = entry.id();
        if !reversed.insert(original_id) {
            return fail("INV-14", format!("entry {original_id} is reversed twice"));
        }
        let Some(original) = ledger.entry(original_id) else {
            return fail("INV-14", format!("reversal {id} of a missing entry"));
        };
        if original.reverses().is_some() {
            return fail("INV-14", format!("reversal {id} reverses a reversal"));
        }
        if original.sequence() >= entry.sequence() {
            return fail("INV-14", format!("reversal {id} precedes its original"));
        }
        let mirrors = original.lines().len() == entry.lines().len()
            && original.lines().iter().zip(entry.lines()).all(|(o, r)| {
                o.account() == r.account() && o.amount() == r.amount() && o.side() != r.side()
            });
        if !mirrors {
            return fail(
                "INV-14",
                format!("reversal {id} does not mirror {original_id}"),
            );
        }
        if ledger.reversal_of(original_id) != Some(id) {
            return fail(
                "INV-14",
                format!("reversal {id} is not recorded on {original_id}"),
            );
        }
    }
    Ok(())
}

fn signed(side: Side, amount: i64) -> i128 {
    match side {
        Side::Debit => i128::from(amount),
        Side::Credit => -i128::from(amount),
    }
}

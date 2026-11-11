//! Journal entries (ADR-0003).
//!
//! An [`EntryDraft`] is what a caller asks for: unvalidated, anything goes.
//! A [`BalancedEntry`] is what the ledger posts. The only way to get one is
//! [`crate::validate`], so code that receives a `BalancedEntry` knows every
//! posting rule already holds ("parse, don't validate").

use std::collections::{BTreeMap, HashMap};

use crate::account::Account;
use crate::chart::Side;
use crate::currency::Currency;
use crate::ids::AccountId;
use crate::money::{Money, PositiveAmount};
use crate::time::Timestamp;

/// Most lines one entry may have.
pub const MAX_LINES: usize = 100;
/// Most metadata keys one entry may have.
pub const MAX_METADATA_KEYS: usize = 32;
/// Most bytes of metadata (keys plus values) one entry may have.
pub const MAX_METADATA_BYTES: usize = 4096;

/// One requested line: post `amount` to `account` on `side`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDraft {
    pub account: AccountId,
    pub side: Side,
    /// Must be greater than zero, in the account's currency.
    pub amount: Money,
}

/// A requested journal entry, not yet validated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryDraft {
    pub lines: Vec<LineDraft>,
    /// Free-form references such as a payment ID or a reason code. Limited
    /// in size; never used for accounting.
    pub metadata: BTreeMap<String, String>,
    /// When the event happened, if the caller knows (for example, an
    /// offline payment). Informational only (INV-12).
    pub event_time: Option<Timestamp>,
    /// Allow two lines with the same account and side. Off by default: such
    /// lines are usually a mistake, and one line with the sum says the same.
    pub allow_duplicate_lines: bool,
}

impl EntryDraft {
    /// Adds a debit line.
    #[must_use]
    pub fn debit(mut self, account: AccountId, amount: Money) -> EntryDraft {
        self.lines.push(LineDraft {
            account,
            side: Side::Debit,
            amount,
        });
        self
    }

    /// Adds a credit line.
    #[must_use]
    pub fn credit(mut self, account: AccountId, amount: Money) -> EntryDraft {
        self.lines.push(LineDraft {
            account,
            side: Side::Credit,
            amount,
        });
        self
    }
}

/// One validated line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    account: AccountId,
    side: Side,
    amount: PositiveAmount,
}

impl Line {
    pub const fn account(&self) -> AccountId {
        self.account
    }

    pub const fn side(&self) -> Side {
        self.side
    }

    pub const fn amount(&self) -> PositiveAmount {
        self.amount
    }
}

/// A journal entry that passed every posting rule: at least two lines, all
/// amounts positive and in their account's currency, all accounts open, and
/// debits equal to credits in every currency (INV-1, INV-3, INV-24).
///
/// It has no public constructor. The only way to obtain one is
/// [`crate::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalancedEntry {
    lines: Vec<Line>,
    metadata: BTreeMap<String, String>,
    event_time: Option<Timestamp>,
}

impl BalancedEntry {
    /// Built only by `validate`, after every rule has passed.
    pub(crate) fn new(
        lines: Vec<Line>,
        metadata: BTreeMap<String, String>,
        event_time: Option<Timestamp>,
    ) -> BalancedEntry {
        BalancedEntry {
            lines,
            metadata,
            event_time,
        }
    }

    pub(crate) const fn line(account: AccountId, side: Side, amount: PositiveAmount) -> Line {
        Line {
            account,
            side,
            amount,
        }
    }

    /// The lines, in the order they were requested.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    pub const fn event_time(&self) -> Option<Timestamp> {
        self.event_time
    }

    /// The total debited in each currency, which equals the total credited.
    pub fn totals(&self) -> BTreeMap<Currency, i64> {
        let mut totals = BTreeMap::new();
        for line in self.lines.iter().filter(|l| l.side == Side::Debit) {
            let amount = line.amount.money();
            // Cannot overflow: validation summed these without overflow.
            *totals.entry(amount.currency()).or_insert(0) += amount.amount();
        }
        totals
    }
}

/// Read access to the accounts an entry refers to. The engine implements it
/// over locked database rows; tests and the simulator use a map.
pub trait AccountsView {
    fn account(&self, id: AccountId) -> Option<&Account>;
}

impl AccountsView for HashMap<AccountId, Account> {
    fn account(&self, id: AccountId) -> Option<&Account> {
        self.get(&id)
    }
}

impl AccountsView for BTreeMap<AccountId, Account> {
    fn account(&self, id: AccountId) -> Option<&Account> {
        self.get(&id)
    }
}

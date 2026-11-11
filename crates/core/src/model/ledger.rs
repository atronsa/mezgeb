//! `MemLedger`: a deliberately simple in-memory ledger.
//!
//! Single-threaded, everything in plain collections, no optimisation. It is
//! the reference the PostgreSQL engine is compared against (differential
//! testing): if the two ever disagree, the engine is wrong.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::account::{Account, AccountStatus, StatusError};
use crate::chart::{Chart, ChartError, Side};
use crate::entry::{AccountsView, BalancedEntry, EntryDraft, LineDraft};
use crate::error::PostingError;
use crate::ids::{AccountId, EntryId};
use crate::money::PositiveAmount;
use crate::time::Timestamp;
use crate::validate::validate;

/// One posted line, with the account's totals right after it (ADR-0012).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedLine {
    pub(super) account: AccountId,
    pub(super) side: Side,
    pub(super) amount: PositiveAmount,
    pub(super) debits_after: i64,
    pub(super) credits_after: i64,
}

impl PostedLine {
    /// A posted line as stored, for rebuilding history from a database.
    pub const fn new(
        account: AccountId,
        side: Side,
        amount: PositiveAmount,
        debits_after: i64,
        credits_after: i64,
    ) -> PostedLine {
        PostedLine {
            account,
            side,
            amount,
            debits_after,
            credits_after,
        }
    }

    pub const fn account(&self) -> AccountId {
        self.account
    }

    pub const fn side(&self) -> Side {
        self.side
    }

    pub const fn amount(&self) -> PositiveAmount {
        self.amount
    }

    /// The account's debit total right after this line.
    pub const fn debits_after(&self) -> i64 {
        self.debits_after
    }

    /// The account's credit total right after this line.
    pub const fn credits_after(&self) -> i64 {
        self.credits_after
    }
}

/// One posted entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedEntry {
    pub(super) id: EntryId,
    pub(super) sequence: u64,
    pub(super) lines: Vec<PostedLine>,
    pub(super) reverses: Option<EntryId>,
    pub(super) metadata: BTreeMap<String, String>,
    pub(super) event_time: Option<Timestamp>,
}

impl PostedEntry {
    /// A posted entry as stored, for rebuilding history from a database.
    /// It is a record of what was posted, not a guarantee: `validate` and
    /// the database constraints are what make posted entries correct.
    pub fn new(
        id: EntryId,
        sequence: u64,
        lines: Vec<PostedLine>,
        reverses: Option<EntryId>,
        metadata: BTreeMap<String, String>,
        event_time: Option<Timestamp>,
    ) -> PostedEntry {
        PostedEntry {
            id,
            sequence,
            lines,
            reverses,
            metadata,
            event_time,
        }
    }

    pub const fn id(&self) -> EntryId {
        self.id
    }

    /// The posting order: 1 for the first entry, then strictly increasing
    /// (INV-12).
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn lines(&self) -> &[PostedLine] {
        &self.lines
    }

    /// The entry this one reverses, if it is a reversal.
    pub const fn reverses(&self) -> Option<EntryId> {
        self.reverses
    }

    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    pub const fn event_time(&self) -> Option<Timestamp> {
        self.event_time
    }
}

/// Why a ledger operation was refused. A refused operation changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    Chart(ChartError),
    DuplicateAccount(AccountId),
    /// An account already exists for this chart reference, such as `"1010"`.
    DuplicateReference(String),
    AccountNotFound(AccountId),
    Status(StatusError),
    DuplicateEntry(EntryId),
    Posting(PostingError),
    EntryNotFound(EntryId),
    /// The entry already has a reversal (INV-14).
    AlreadyReversed(EntryId),
    /// A reversal can never itself be reversed (INV-14).
    CannotReverseReversal(EntryId),
    /// The storage behind a ledger failed, or returned data that breaks the
    /// ledger's rules. Never produced by `MemLedger`.
    Storage(String),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::Chart(e) => e.fmt(f),
            LedgerError::DuplicateAccount(id) => write!(f, "account {id} already exists"),
            LedgerError::DuplicateReference(reference) => {
                write!(f, "an account for {reference} already exists")
            }
            LedgerError::AccountNotFound(id) => write!(f, "account {id} not found"),
            LedgerError::Status(e) => e.fmt(f),
            LedgerError::DuplicateEntry(id) => write!(f, "entry {id} already exists"),
            LedgerError::Posting(e) => e.fmt(f),
            LedgerError::EntryNotFound(id) => write!(f, "entry {id} not found"),
            LedgerError::AlreadyReversed(id) => write!(f, "entry {id} is already reversed"),
            LedgerError::CannotReverseReversal(id) => {
                write!(f, "entry {id} is a reversal and cannot be reversed")
            }
            LedgerError::Storage(message) => write!(f, "storage error: {message}"),
        }
    }
}

impl std::error::Error for LedgerError {}

/// The in-memory reference ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemLedger {
    pub(super) accounts: BTreeMap<AccountId, Account>,
    pub(super) entries: Vec<PostedEntry>,
    pub(super) index: HashMap<EntryId, usize>,
    pub(super) reversed_by: HashMap<EntryId, EntryId>,
    /// Test only: skip the totals update of each entry's last line, to prove
    /// the invariant checker and the state-machine test catch it.
    #[cfg(test)]
    pub(super) planted_bug: bool,
}

impl AccountsView for MemLedger {
    fn account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.get(&id)
    }
}

impl MemLedger {
    pub fn new() -> MemLedger {
        MemLedger::default()
    }

    /// Opens an account for a chart reference such as `"2010:cust-abebe"`.
    pub fn open_account(
        &mut self,
        id: AccountId,
        chart: &Chart,
        reference: &str,
    ) -> Result<&Account, LedgerError> {
        if self.accounts.contains_key(&id) {
            return Err(LedgerError::DuplicateAccount(id));
        }
        let account = Account::open(id, chart, reference).map_err(LedgerError::Chart)?;
        let reference = account.reference();
        if self.accounts.values().any(|a| a.reference() == reference) {
            return Err(LedgerError::DuplicateReference(reference));
        }
        Ok(self.accounts.entry(id).or_insert(account))
    }

    /// Freezes, unfreezes or closes an account.
    pub fn set_status(&mut self, id: AccountId, to: AccountStatus) -> Result<(), LedgerError> {
        let account = self
            .accounts
            .get_mut(&id)
            .ok_or(LedgerError::AccountNotFound(id))?;
        account.set_status(to).map_err(LedgerError::Status)
    }

    /// Validates and posts an entry.
    pub fn post(&mut self, id: EntryId, draft: EntryDraft) -> Result<&PostedEntry, LedgerError> {
        if self.index.contains_key(&id) {
            return Err(LedgerError::DuplicateEntry(id));
        }
        let entry = validate(draft, self).map_err(LedgerError::Posting)?;
        let position = self.apply(id, entry, None)?;
        self.posted(id, position)
    }

    /// Posts the exact mirror of `original`: same accounts and amounts,
    /// opposite sides (INV-14). The reversal itself must pass every posting
    /// rule, so it fails if, for example, the money was already spent.
    pub fn reverse(&mut self, original: EntryId, id: EntryId) -> Result<&PostedEntry, LedgerError> {
        if self.index.contains_key(&id) {
            return Err(LedgerError::DuplicateEntry(id));
        }
        let entry = self
            .entry(original)
            .ok_or(LedgerError::EntryNotFound(original))?;
        if entry.reverses.is_some() {
            return Err(LedgerError::CannotReverseReversal(original));
        }
        if self.reversed_by.contains_key(&original) {
            return Err(LedgerError::AlreadyReversed(original));
        }
        let mirror = EntryDraft {
            lines: entry
                .lines
                .iter()
                .map(|line| LineDraft {
                    account: line.account,
                    side: opposite(line.side),
                    amount: line.amount.money(),
                })
                .collect(),
            // The original passed the duplicate rule, or explicitly allowed
            // duplicates; its mirror must not be refused for that.
            allow_duplicate_lines: true,
            ..EntryDraft::default()
        };
        let reversal = validate(mirror, self).map_err(LedgerError::Posting)?;
        let position = self.apply(id, reversal, Some(original))?;
        self.reversed_by.insert(original, id);
        self.posted(id, position)
    }

    fn posted(&self, id: EntryId, position: usize) -> Result<&PostedEntry, LedgerError> {
        self.entries
            .get(position)
            .ok_or(LedgerError::EntryNotFound(id))
    }

    /// Writes a validated entry and returns its position. Totals are
    /// computed on copies first, so nothing changes unless every line
    /// applies.
    fn apply(
        &mut self,
        id: EntryId,
        entry: BalancedEntry,
        reverses: Option<EntryId>,
    ) -> Result<usize, LedgerError> {
        let mut updated: BTreeMap<AccountId, Account> = BTreeMap::new();
        let mut lines = Vec::with_capacity(entry.lines().len());
        let last = entry.lines().len().saturating_sub(1);
        for (i, line) in entry.lines().iter().enumerate() {
            let account_id = line.account();
            let account = match updated.entry(account_id) {
                std::collections::btree_map::Entry::Occupied(slot) => slot.into_mut(),
                std::collections::btree_map::Entry::Vacant(slot) => slot.insert(
                    self.accounts
                        .get(&account_id)
                        .ok_or(LedgerError::AccountNotFound(account_id))?
                        .clone(),
                ),
            };
            if !self.skips_line(i, last) {
                account
                    .record(line.side(), line.amount().amount())
                    .ok_or(LedgerError::Posting(PostingError::AccountTotalOverflow {
                        line: i,
                    }))?;
            }
            lines.push(PostedLine {
                account: account_id,
                side: line.side(),
                amount: line.amount(),
                debits_after: account.debits_total(),
                credits_after: account.credits_total(),
            });
        }
        self.accounts.extend(updated);
        let position = self.entries.len();
        self.entries.push(PostedEntry {
            id,
            sequence: position as u64 + 1,
            lines,
            reverses,
            metadata: entry.metadata().clone(),
            event_time: entry.event_time(),
        });
        self.index.insert(id, position);
        Ok(position)
    }

    #[cfg(test)]
    fn skips_line(&self, line: usize, last: usize) -> bool {
        self.planted_bug && line == last
    }

    #[cfg(not(test))]
    fn skips_line(&self, _line: usize, _last: usize) -> bool {
        false
    }

    pub fn account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.get(&id)
    }

    /// Every account, by ID.
    pub fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    /// Every posted entry, in posting order.
    pub fn entries(&self) -> &[PostedEntry] {
        &self.entries
    }

    pub fn entry(&self, id: EntryId) -> Option<&PostedEntry> {
        self.index.get(&id).and_then(|&i| self.entries.get(i))
    }

    /// The reversal of an entry, if it has one.
    pub fn reversal_of(&self, id: EntryId) -> Option<EntryId> {
        self.reversed_by.get(&id).copied()
    }

    /// Every line posted to an account, in posting order, with its entry.
    pub fn history(&self, account: AccountId) -> Vec<(&PostedEntry, &PostedLine)> {
        self.entries
            .iter()
            .flat_map(|e| e.lines.iter().map(move |l| (e, l)))
            .filter(|(_, l)| l.account == account)
            .collect()
    }
}

const fn opposite(side: Side) -> Side {
    match side {
        Side::Debit => Side::Credit,
        Side::Credit => Side::Debit,
    }
}

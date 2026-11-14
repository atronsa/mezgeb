//! `MemLedger`: a deliberately simple in-memory ledger.
//!
//! Single-threaded, everything in plain collections, no optimisation. It is
//! the reference the PostgreSQL engine is compared against (differential
//! testing): if the two ever disagree, the engine is wrong.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::account::{Account, AccountStatus, StatusError};
use crate::authorization::{Authorization, EntryKind};
use crate::chart::{Chart, ChartError, Side};
use crate::entry::{AccountsView, BalancedEntry, EntryDraft, LineDraft};
use crate::error::PostingError;
use crate::hold::{Hold, HoldStatus};
use crate::ids::{AccountId, EntryId, HoldId};
use crate::money::PositiveAmount;
use crate::refund::RefundLimits;
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
    pub(super) kind: EntryKind,
    pub(super) authorization: Option<Authorization>,
    pub(super) refunds: Option<EntryId>,
}

impl PostedEntry {
    /// A posted entry as stored, for rebuilding history from a database.
    /// It is a record of what was posted, not a guarantee: `validate` and
    /// the database constraints are what make posted entries correct.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: EntryId,
        sequence: u64,
        lines: Vec<PostedLine>,
        reverses: Option<EntryId>,
        metadata: BTreeMap<String, String>,
        event_time: Option<Timestamp>,
        kind: EntryKind,
        authorization: Option<Authorization>,
    ) -> PostedEntry {
        PostedEntry {
            id,
            sequence,
            lines,
            reverses,
            metadata,
            event_time,
            kind,
            authorization,
            refunds: None,
        }
    }

    /// The same entry, as a refund of `original` (when rebuilding a refund
    /// from storage).
    #[must_use]
    pub const fn refunding(mut self, original: Option<EntryId>) -> PostedEntry {
        self.refunds = original;
        self
    }

    /// The entry this one refunds, if it is a refund.
    pub const fn refunds(&self) -> Option<EntryId> {
        self.refunds
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

    /// Standard, reversal, adjustment or refund.
    pub const fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Who asked for and approved a reversal or adjustment, and why.
    pub fn authorization(&self) -> Option<&Authorization> {
        self.authorization.as_ref()
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
    /// The operation conflicted with concurrent operations (a deadlock or a
    /// serialization failure, with its SQLSTATE) and still failed after the
    /// engine's retries. Safe to retry later. Never produced by `MemLedger`.
    Contention(String),
    /// An account stayed locked by other operations longer than the lock
    /// timeout. Safe to retry later. Never produced by `MemLedger`.
    Busy,
    /// The idempotency key was already used by this client for a different
    /// request (ADR-0008, INV-13). Never produced by `MemLedger`.
    IdempotencyKeyReused,
    /// A reversal or adjustment needs a reason and a requester, and an
    /// adjustment also an approver (INV-25).
    AuthorizationRequired,
    /// A standard entry carries no reason or approver.
    AuthorizationNotAllowed,
    /// The reason code is unknown, or not for this kind of entry.
    ReasonNotAllowed(String),
    /// The approver is the same person as the requester (INV-25).
    MakerIsChecker,
    /// A hold with this ID already exists.
    DuplicateHold(HoldId),
    HoldNotFound(HoldId),
    /// The hold has already ended (INV-16).
    HoldNotActive {
        hold: HoldId,
        status: HoldStatus,
    },
    /// The hold's expiry time has passed: it can only be expired now.
    HoldExpired(HoldId),
    /// The hold's expiry time has not passed yet.
    HoldNotExpired(HoldId),
    /// The capture is larger than the hold (INV-17).
    CaptureExceedsHold(HoldId),
    /// The held account must be a different, unused account of the same
    /// type, with a zero balance.
    InvalidHeldAccount(AccountId),
    /// The account is a hold's held account: only that hold's own entries
    /// may post to it (INV-18).
    AccountIsHeld(AccountId),
    /// Only a standard entry can be refunded: not a reversal, an adjustment
    /// or another refund (INV-15).
    NotRefundable(EntryId),
    /// The entry has been reversed, so there is nothing left to refund
    /// (INV-15).
    RefundOfReversedEntry(EntryId),
    /// A refund line moves an account the wrong way: it credits an account
    /// the original did not debit, or debits an account the original
    /// debited (INV-15). `line` is the zero-based line.
    RefundLineNotAllowed {
        line: usize,
    },
    /// The refunds of an entry would move an account back by more than the
    /// original moved it (INV-15). `line` is the first line on that account.
    RefundExceedsOriginal {
        line: usize,
    },
    /// The entry has refunds, so a reversal would give the money back twice
    /// (INV-15).
    RefundedEntryCannotBeReversed(EntryId),
    /// A refund is never reversed; a mistaken refund is corrected with an
    /// adjustment (INV-15).
    CannotReverseRefund(EntryId),
    /// An adjustment cannot also be a refund.
    RefundAsAdjustment,
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
            LedgerError::Contention(code) => {
                write!(
                    f,
                    "conflicted with concurrent operations ({code}); retry later"
                )
            }
            LedgerError::Busy => f.write_str("accounts are busy; retry later"),
            LedgerError::IdempotencyKeyReused => {
                f.write_str("the idempotency key was already used for a different request")
            }
            LedgerError::AuthorizationRequired => f.write_str(
                "a reason and a requester (and, for an adjustment, an approver) are required",
            ),
            LedgerError::AuthorizationNotAllowed => {
                f.write_str("a standard entry has no reason or approver")
            }
            LedgerError::ReasonNotAllowed(code) => {
                write!(
                    f,
                    "reason {code:?} is unknown or not for this kind of entry"
                )
            }
            LedgerError::MakerIsChecker => {
                f.write_str("the approver must be a different person from the requester")
            }
            LedgerError::DuplicateHold(id) => write!(f, "hold {id} already exists"),
            LedgerError::HoldNotFound(id) => write!(f, "hold {id} not found"),
            LedgerError::HoldNotActive { hold, status } => {
                write!(f, "hold {hold} is already {status}")
            }
            LedgerError::HoldExpired(id) => write!(f, "hold {id} has expired"),
            LedgerError::HoldNotExpired(id) => write!(f, "hold {id} has not expired yet"),
            LedgerError::CaptureExceedsHold(id) => {
                write!(f, "the capture is larger than hold {id}")
            }
            LedgerError::InvalidHeldAccount(id) => write!(
                f,
                "account {id} cannot hold money: it must be unused, empty and of the same type"
            ),
            LedgerError::NotRefundable(id) => {
                write!(
                    f,
                    "entry {id} is not a standard entry and cannot be refunded"
                )
            }
            LedgerError::RefundOfReversedEntry(id) => {
                write!(f, "entry {id} has been reversed and cannot be refunded")
            }
            LedgerError::RefundLineNotAllowed { line } => write!(
                f,
                "refund line {line} moves an account the original did not move that way"
            ),
            LedgerError::RefundExceedsOriginal { line } => write!(
                f,
                "refund line {line} would give back more than the original moved"
            ),
            LedgerError::RefundedEntryCannotBeReversed(id) => {
                write!(f, "entry {id} has refunds and cannot be reversed")
            }
            LedgerError::CannotReverseRefund(id) => {
                write!(f, "entry {id} is a refund and cannot be reversed")
            }
            LedgerError::RefundAsAdjustment => f.write_str("an adjustment cannot be a refund"),
            LedgerError::AccountIsHeld(id) => {
                write!(
                    f,
                    "account {id} belongs to a hold and only its hold may post to it"
                )
            }
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
    /// For each refunded entry, what its refunds moved back so far, per
    /// account (INV-15).
    pub(super) refunded: HashMap<EntryId, BTreeMap<AccountId, i64>>,
    pub(super) holds: BTreeMap<HoldId, Hold>,
    /// Each held account, and the hold it belongs to.
    pub(super) held_accounts: BTreeMap<AccountId, HoldId>,
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
        self.refuse_held(draft.lines.iter().map(|l| l.account))?;
        let Some(original) = draft.refunds else {
            let entry = validate(draft, self).map_err(LedgerError::Posting)?;
            let position = self.apply(id, entry, None, EntryKind::Standard, None)?;
            return self.posted(id, position);
        };

        // A refund (INV-15): checked against the original and its earlier
        // refunds first, then against the posting rules.
        let entry = self
            .entry(original)
            .ok_or(LedgerError::EntryNotFound(original))?;
        if entry.kind != EntryKind::Standard {
            return Err(LedgerError::NotRefundable(original));
        }
        if self.reversed_by.contains_key(&original) {
            return Err(LedgerError::RefundOfReversedEntry(original));
        }
        let before = self.refunded.get(&original).cloned().unwrap_or_default();
        let after = RefundLimits::of(entry).check(&draft.lines, &before)?;
        let refund = validate(draft, self).map_err(LedgerError::Posting)?;
        let position = self.apply(id, refund, None, EntryKind::Refund, None)?;
        self.refunded.entry(original).or_default().extend(after);
        self.posted(id, position)
    }

    /// What the refunds of `original` have moved back so far, per account.
    pub fn refunded(&self, original: EntryId) -> BTreeMap<AccountId, i64> {
        self.refunded.get(&original).cloned().unwrap_or_default()
    }

    /// Validates and posts an adjustment: a manual correction with a reason
    /// and a different requester and approver (INV-25), such as E14's
    /// goodwill credit.
    pub fn post_adjustment(
        &mut self,
        id: EntryId,
        draft: EntryDraft,
        authorization: Authorization,
    ) -> Result<&PostedEntry, LedgerError> {
        if self.index.contains_key(&id) {
            return Err(LedgerError::DuplicateEntry(id));
        }
        Authorization::check(EntryKind::Adjustment, Some(&authorization))?;
        if draft.refunds.is_some() {
            return Err(LedgerError::RefundAsAdjustment);
        }
        self.refuse_held(draft.lines.iter().map(|l| l.account))?;
        let entry = validate(draft, self).map_err(LedgerError::Posting)?;
        let position = self.apply(id, entry, None, EntryKind::Adjustment, Some(authorization))?;
        self.posted(id, position)
    }

    /// Posts the exact mirror of `original`: same accounts and amounts,
    /// opposite sides (INV-14), with a reason and a requester. The reversal
    /// itself must pass every posting rule, so it fails if, for example, the
    /// money was already spent; that mistake is then corrected with an
    /// adjustment (ADR-0013).
    pub fn reverse(
        &mut self,
        original: EntryId,
        id: EntryId,
        authorization: Authorization,
    ) -> Result<&PostedEntry, LedgerError> {
        if self.index.contains_key(&id) {
            return Err(LedgerError::DuplicateEntry(id));
        }
        Authorization::check(EntryKind::Reversal, Some(&authorization))?;
        let entry = self
            .entry(original)
            .ok_or(LedgerError::EntryNotFound(original))?;
        if entry.reverses.is_some() {
            return Err(LedgerError::CannotReverseReversal(original));
        }
        if entry.kind == EntryKind::Refund {
            return Err(LedgerError::CannotReverseRefund(original));
        }
        if self
            .refunded
            .get(&original)
            .is_some_and(|r| r.values().any(|moved| *moved > 0))
        {
            return Err(LedgerError::RefundedEntryCannotBeReversed(original));
        }
        if self.reversed_by.contains_key(&original) {
            return Err(LedgerError::AlreadyReversed(original));
        }
        self.refuse_held(entry.lines.iter().map(|l| l.account))?;
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
        let position = self.apply(
            id,
            reversal,
            Some(original),
            EntryKind::Reversal,
            Some(authorization),
        )?;
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
        kind: EntryKind,
        authorization: Option<Authorization>,
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
            kind,
            authorization,
            refunds: entry.refunds(),
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

impl MemLedger {
    /// Refuses ordinary postings to a hold's held account (INV-18).
    fn refuse_held(
        &self,
        mut accounts: impl Iterator<Item = AccountId>,
    ) -> Result<(), LedgerError> {
        match accounts.find(|a| self.held_accounts.contains_key(a)) {
            Some(account) => Err(LedgerError::AccountIsHeld(account)),
            None => Ok(()),
        }
    }

    /// Validates and posts one of a hold's own entries, tagged with the hold
    /// and the action in its metadata.
    fn post_hold_entry(
        &mut self,
        id: EntryId,
        hold: HoldId,
        action: &str,
        lines: Vec<LineDraft>,
    ) -> Result<(), LedgerError> {
        if self.index.contains_key(&id) {
            return Err(LedgerError::DuplicateEntry(id));
        }
        let draft = EntryDraft {
            lines,
            metadata: BTreeMap::from([
                ("hold".to_owned(), hold.to_string()),
                ("hold_action".to_owned(), action.to_owned()),
            ]),
            // A capture back to the reserving account repeats its side.
            allow_duplicate_lines: true,
            ..EntryDraft::default()
        };
        let entry = validate(draft, self).map_err(LedgerError::Posting)?;
        self.apply(id, entry, None, EntryKind::Standard, None)?;
        Ok(())
    }

    /// The active hold `id`, or why it cannot be used.
    fn active_hold(&self, id: HoldId) -> Result<Hold, LedgerError> {
        let hold = self.holds.get(&id).ok_or(LedgerError::HoldNotFound(id))?;
        if hold.status() != HoldStatus::Active {
            return Err(LedgerError::HoldNotActive {
                hold: id,
                status: hold.status(),
            });
        }
        Ok(hold.clone())
    }

    /// The side on which an account's balance increases.
    fn increase_side(&self, account: AccountId) -> Result<Side, LedgerError> {
        self.accounts
            .get(&account)
            .map(Account::normal_side)
            .ok_or(LedgerError::AccountNotFound(account))
    }

    /// Places a hold: moves `amount` from `account` into `held`, the hold's
    /// own account (E02, E16). `held` must be open, unused, empty and of the
    /// same type as `account`.
    pub fn place_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        account: AccountId,
        held: AccountId,
        amount: PositiveAmount,
        expires_at: Timestamp,
    ) -> Result<&Hold, LedgerError> {
        if self.holds.contains_key(&id) {
            return Err(LedgerError::DuplicateHold(id));
        }
        self.refuse_held([account].into_iter())?;
        let source = self
            .accounts
            .get(&account)
            .ok_or(LedgerError::AccountNotFound(account))?;
        let target = self
            .accounts
            .get(&held)
            .ok_or(LedgerError::AccountNotFound(held))?;
        let usable = held != account
            && !self.held_accounts.contains_key(&held)
            && target.debits_total() == 0
            && target.credits_total() == 0
            && target.account_type() == source.account_type();
        if !usable {
            return Err(LedgerError::InvalidHeldAccount(held));
        }
        let side = source.normal_side();
        let money = amount.money();
        self.post_hold_entry(
            entry,
            id,
            "place",
            vec![
                LineDraft {
                    account,
                    side: opposite(side),
                    amount: money,
                },
                LineDraft {
                    account: held,
                    side,
                    amount: money,
                },
            ],
        )?;
        self.holds.insert(
            id,
            Hold::new(
                id,
                account,
                held,
                amount,
                0,
                HoldStatus::Active,
                expires_at,
                entry,
                None,
            ),
        );
        self.held_accounts.insert(held, id);
        self.holds.get(&id).ok_or(LedgerError::HoldNotFound(id))
    }

    /// Captures `amount` of an active, unexpired hold into `destination`
    /// (E03, E17). Whatever is not captured goes back to the account in the
    /// same entry, so a hold is captured at most once (INV-16, INV-17).
    pub fn capture_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        destination: AccountId,
        amount: PositiveAmount,
        now: Timestamp,
    ) -> Result<&Hold, LedgerError> {
        let hold = self.active_hold(id)?;
        if now >= hold.expires_at() {
            return Err(LedgerError::HoldExpired(id));
        }
        if amount.amount() > hold.amount().amount() {
            return Err(LedgerError::CaptureExceedsHold(id));
        }
        self.refuse_held([destination].into_iter())?;
        let side = self.increase_side(hold.held_account())?;
        let total = hold.amount().money();
        let remainder = hold.amount().amount() - amount.amount();
        let mut lines = vec![
            LineDraft {
                account: hold.held_account(),
                side: opposite(side),
                amount: total,
            },
            LineDraft {
                account: destination,
                side,
                amount: amount.money(),
            },
        ];
        if remainder > 0 {
            lines.push(LineDraft {
                account: hold.account(),
                side,
                amount: crate::money::Money::from_minor(remainder, total.currency()),
            });
        }
        self.post_hold_entry(entry, id, "capture", lines)?;
        self.close_hold(id, HoldStatus::Captured, amount.amount(), entry)
    }

    /// Releases an active hold: everything goes back to the account (E04).
    pub fn release_hold(&mut self, id: HoldId, entry: EntryId) -> Result<&Hold, LedgerError> {
        let hold = self.active_hold(id)?;
        self.return_hold(&hold, entry, "release")?;
        self.close_hold(id, HoldStatus::Released, 0, entry)
    }

    /// Expires an active hold whose expiry time has passed: everything goes
    /// back to the account (E18).
    pub fn expire_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        now: Timestamp,
    ) -> Result<&Hold, LedgerError> {
        let hold = self.active_hold(id)?;
        if now < hold.expires_at() {
            return Err(LedgerError::HoldNotExpired(id));
        }
        self.return_hold(&hold, entry, "expire")?;
        self.close_hold(id, HoldStatus::Expired, 0, entry)
    }

    fn return_hold(
        &mut self,
        hold: &Hold,
        entry: EntryId,
        action: &str,
    ) -> Result<(), LedgerError> {
        let side = self.increase_side(hold.held_account())?;
        let money = hold.amount().money();
        self.post_hold_entry(
            entry,
            hold.id(),
            action,
            vec![
                LineDraft {
                    account: hold.held_account(),
                    side: opposite(side),
                    amount: money,
                },
                LineDraft {
                    account: hold.account(),
                    side,
                    amount: money,
                },
            ],
        )
    }

    fn close_hold(
        &mut self,
        id: HoldId,
        status: HoldStatus,
        captured: i64,
        entry: EntryId,
    ) -> Result<&Hold, LedgerError> {
        let hold = self
            .holds
            .get_mut(&id)
            .ok_or(LedgerError::HoldNotFound(id))?;
        hold.close(status, captured, entry);
        Ok(hold)
    }

    pub fn hold(&self, id: HoldId) -> Option<&Hold> {
        self.holds.get(&id)
    }

    /// Every hold, by ID.
    pub fn holds(&self) -> impl Iterator<Item = &Hold> {
        self.holds.values()
    }
}

const fn opposite(side: Side) -> Side {
    match side {
        Side::Debit => Side::Credit,
        Side::Credit => Side::Debit,
    }
}

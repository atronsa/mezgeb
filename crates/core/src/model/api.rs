//! The operations every ledger offers, whatever it stores data in.
//!
//! `MemLedger` implements it in memory; the engine's `PgStore` implements it
//! on PostgreSQL. Tests run the same operations on both and compare the
//! results (differential testing, step 1.09).

use std::future::{Future, ready};

use crate::account::{Account, AccountStatus};
use crate::authorization::Authorization;
use crate::chart::Chart;
use crate::entry::EntryDraft;
use crate::hold::Hold;
use crate::ids::HoldId;
use crate::ids::{AccountId, EntryId};
use crate::model::ledger::{LedgerError, MemLedger, PostedEntry};
use crate::money::PositiveAmount;
use crate::time::Timestamp;

/// A ledger: open accounts, post and reverse entries, read them back.
///
/// Every method is all-or-nothing: an operation that returns an error has
/// changed nothing.
pub trait Ledger {
    /// Opens an account for a chart reference such as `"2010:cust-abebe"`.
    fn open_account(
        &mut self,
        id: AccountId,
        chart: &Chart,
        reference: &str,
    ) -> impl Future<Output = Result<Account, LedgerError>> + Send;

    /// Freezes, unfreezes or closes an account.
    fn set_status(
        &mut self,
        id: AccountId,
        to: AccountStatus,
    ) -> impl Future<Output = Result<(), LedgerError>> + Send;

    /// Validates and posts an entry.
    fn post(
        &mut self,
        id: EntryId,
        draft: EntryDraft,
    ) -> impl Future<Output = Result<PostedEntry, LedgerError>> + Send;

    /// Posts an adjustment, approved by a second person (INV-25).
    fn post_adjustment(
        &mut self,
        id: EntryId,
        draft: EntryDraft,
        authorization: Authorization,
    ) -> impl Future<Output = Result<PostedEntry, LedgerError>> + Send;

    /// Posts the exact mirror of `original`, with a reason (INV-14).
    fn reverse(
        &mut self,
        original: EntryId,
        id: EntryId,
        authorization: Authorization,
    ) -> impl Future<Output = Result<PostedEntry, LedgerError>> + Send;

    /// Reads one account.
    fn get_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = Result<Option<Account>, LedgerError>> + Send;

    /// Reads every account, ordered by ID.
    fn all_accounts(&mut self) -> impl Future<Output = Result<Vec<Account>, LedgerError>> + Send;

    /// Reads every entry, in posting order.
    fn all_entries(&mut self)
    -> impl Future<Output = Result<Vec<PostedEntry>, LedgerError>> + Send;
}

impl Ledger for MemLedger {
    fn open_account(
        &mut self,
        id: AccountId,
        chart: &Chart,
        reference: &str,
    ) -> impl Future<Output = Result<Account, LedgerError>> + Send {
        ready(MemLedger::open_account(self, id, chart, reference).cloned())
    }

    fn set_status(
        &mut self,
        id: AccountId,
        to: AccountStatus,
    ) -> impl Future<Output = Result<(), LedgerError>> + Send {
        ready(MemLedger::set_status(self, id, to))
    }

    fn post(
        &mut self,
        id: EntryId,
        draft: EntryDraft,
    ) -> impl Future<Output = Result<PostedEntry, LedgerError>> + Send {
        ready(MemLedger::post(self, id, draft).cloned())
    }

    fn post_adjustment(
        &mut self,
        id: EntryId,
        draft: EntryDraft,
        authorization: Authorization,
    ) -> impl Future<Output = Result<PostedEntry, LedgerError>> + Send {
        ready(MemLedger::post_adjustment(self, id, draft, authorization).cloned())
    }

    fn reverse(
        &mut self,
        original: EntryId,
        id: EntryId,
        authorization: Authorization,
    ) -> impl Future<Output = Result<PostedEntry, LedgerError>> + Send {
        ready(MemLedger::reverse(self, original, id, authorization).cloned())
    }

    fn get_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = Result<Option<Account>, LedgerError>> + Send {
        ready(Ok(self.account(id).cloned()))
    }

    fn all_accounts(&mut self) -> impl Future<Output = Result<Vec<Account>, LedgerError>> + Send {
        ready(Ok(self.accounts().cloned().collect()))
    }

    fn all_entries(
        &mut self,
    ) -> impl Future<Output = Result<Vec<PostedEntry>, LedgerError>> + Send {
        ready(Ok(self.entries().to_vec()))
    }
}

/// Hold operations (ADR-0004, step 1.14): reserve money now, capture or
/// release it later. Each operation is all-or-nothing.
pub trait Holds {
    /// Moves `amount` from `account` into `held`, the hold's own account.
    #[allow(clippy::too_many_arguments)]
    fn place_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        account: AccountId,
        held: AccountId,
        amount: PositiveAmount,
        expires_at: Timestamp,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send;

    /// Captures `amount` of an active hold into `destination`; the rest
    /// returns to the account in the same entry.
    fn capture_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        destination: AccountId,
        amount: PositiveAmount,
        now: Timestamp,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send;

    /// Returns everything to the account.
    fn release_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send;

    /// Returns everything to the account once the hold has expired.
    fn expire_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        now: Timestamp,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send;

    /// Reads every hold, ordered by ID.
    fn all_holds(&mut self) -> impl Future<Output = Result<Vec<Hold>, LedgerError>> + Send;
}

impl Holds for MemLedger {
    fn place_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        account: AccountId,
        held: AccountId,
        amount: PositiveAmount,
        expires_at: Timestamp,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send {
        ready(MemLedger::place_hold(self, id, entry, account, held, amount, expires_at).cloned())
    }

    fn capture_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        destination: AccountId,
        amount: PositiveAmount,
        now: Timestamp,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send {
        ready(MemLedger::capture_hold(self, id, entry, destination, amount, now).cloned())
    }

    fn release_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send {
        ready(MemLedger::release_hold(self, id, entry).cloned())
    }

    fn expire_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        now: Timestamp,
    ) -> impl Future<Output = Result<Hold, LedgerError>> + Send {
        ready(MemLedger::expire_hold(self, id, entry, now).cloned())
    }

    fn all_holds(&mut self) -> impl Future<Output = Result<Vec<Hold>, LedgerError>> + Send {
        ready(Ok(self.holds().cloned().collect()))
    }
}

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
use crate::ids::{AccountId, EntryId};
use crate::model::ledger::{LedgerError, MemLedger, PostedEntry};

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

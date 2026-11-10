//! The state machine (spec §4). Storage-agnostic: the in-memory simulator and
//! the PostgreSQL store both implement `StateView`.
//!
//! **Scaffold, to be replaced.** This module follows the draft one-debit,
//! one-credit transfer model. ADR-0003 (multi-line journal entries) and
//! ADR-0004 (holds as postings) replace it in steps 1.04–1.06.

use crate::account::Account;
use crate::error::{CreateAccountError, CreateTransferError};
use crate::id::{AccountId, Timestamp, TransferId};
use crate::transfer::Transfer;

/// Read/write access to ledger state inside one atomic batch.
pub trait StateView {
    fn account(&self, id: AccountId) -> Option<&Account>;
    fn transfer(&self, id: TransferId) -> Option<&Transfer>;
    fn put_account(&mut self, account: Account);
    fn put_transfer(&mut self, transfer: Transfer);
}

pub struct Engine;

impl Engine {
    /// Apply a batch of account creations. One result per input, same order.
    pub fn create_accounts<S: StateView>(
        _state: &mut S,
        _accounts: &[Account],
        _now: Timestamp,
    ) -> Vec<Result<(), CreateAccountError>> {
        todo!("spec §3.4: validate, check idempotency, insert")
    }

    /// Apply a batch of transfers. One result per input, same order.
    /// Linked chains are all-or-nothing (spec §4.4).
    pub fn create_transfers<S: StateView>(
        _state: &mut S,
        _transfers: &[Transfer],
        _now: Timestamp,
    ) -> Vec<Result<(), CreateTransferError>> {
        todo!("spec §4.1-§4.7")
    }

    /// Void pending transfers whose timeout has passed (spec §4.3).
    pub fn expire_pending<S: StateView>(_state: &mut S, _now: Timestamp) -> usize {
        todo!("spec §4.3")
    }
}

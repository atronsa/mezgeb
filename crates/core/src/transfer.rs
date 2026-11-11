//! Transfers (spec §3.5, §4).
//!
//! **Scaffold, to be replaced.** This module follows the draft one-debit,
//! one-credit transfer model. ADR-0003 (multi-line journal entries) and
//! ADR-0004 (holds as postings) replace it in steps 1.04–1.06.

use crate::id::{Amount, LedgerId, Timestamp, TransferId};
use crate::ids::AccountId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferFlags(pub u16);

impl TransferFlags {
    /// This transfer succeeds or fails together with the next one in the batch.
    pub const LINKED: u16 = 1 << 0;
    /// Reserve funds (two-phase step 1).
    pub const PENDING: u16 = 1 << 1;
    /// Commit a pending transfer, fully or partially.
    pub const POST_PENDING: u16 = 1 << 2;
    /// Release a pending transfer.
    pub const VOID_PENDING: u16 = 1 << 3;
    /// Transfer at most `amount`, limited by the debit account's balance.
    pub const BALANCING_DEBIT: u16 = 1 << 4;
    /// Transfer at most `amount`, limited by the credit account's balance.
    pub const BALANCING_CREDIT: u16 = 1 << 5;

    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub id: TransferId,
    pub debit_account_id: AccountId,
    pub credit_account_id: AccountId,
    pub amount: Amount,
    pub ledger: LedgerId,
    pub code: u16,
    pub flags: TransferFlags,
    /// For POST_PENDING / VOID_PENDING: the pending transfer being resolved.
    pub pending_id: Option<TransferId>,
    /// For PENDING: seconds until the hold expires (0 = never).
    pub timeout: u32,
    pub user_data: u128,
    /// Claimed by the client/device. Informational only.
    pub event_time: Option<Timestamp>,
    /// Assigned by the ledger. Authoritative; strictly monotonic.
    pub recorded_time: Timestamp,
}

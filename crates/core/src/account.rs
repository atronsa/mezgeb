//! Accounts (spec §3.4).
//!
//! **Scaffold, to be replaced.** This module follows the draft one-debit,
//! one-credit transfer model. ADR-0003 (multi-line journal entries) and
//! ADR-0004 (holds as postings) replace it in steps 1.04–1.06.

use crate::id::{Amount, LedgerId, Timestamp};
use crate::ids::AccountId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccountFlags(pub u16);

impl AccountFlags {
    pub const DEBITS_MUST_NOT_EXCEED_CREDITS: u16 = 1 << 0;
    pub const CREDITS_MUST_NOT_EXCEED_DEBITS: u16 = 1 << 1;
    pub const CLOSED: u16 = 1 << 2;
    pub const HISTORY: u16 = 1 << 3;

    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub ledger: LedgerId,
    /// Client-defined category (chart of accounts).
    pub code: u16,
    pub flags: AccountFlags,
    pub debits_pending: Amount,
    pub debits_posted: Amount,
    pub credits_pending: Amount,
    pub credits_posted: Amount,
    /// Opaque reference back to the client's own records.
    pub user_data: u128,
    pub created_at: Timestamp,
}

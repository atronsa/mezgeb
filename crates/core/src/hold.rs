//! Holds: reserving money now, capturing or releasing it later (ADR-0004,
//! step 1.14).
//!
//! A hold is a real posting. Placing it moves the amount from an account
//! into the hold's own held account, so the money can no longer be spent
//! but stays visible in the books (E02, E16). The hold then ends exactly
//! once (INV-16):
//!
//! - **captured**: the captured part moves from the held account to a
//!   destination (E03, E17), and any remainder goes back to the account,
//!   in the same entry (INV-17);
//! - **released**: everything goes back to the account (E04);
//! - **expired**: the same as released, after the hold's expiry time (E18).
//!
//! Each hold has its own held account, used by nothing else, so the held
//! account's balance is the hold's amount while it is active and zero once
//! it has ended (INV-18).

use std::fmt;

use crate::ids::{AccountId, EntryId, HoldId};
use crate::money::PositiveAmount;
use crate::time::Timestamp;

/// Where a hold is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldStatus {
    Active,
    Captured,
    Released,
    Expired,
}

impl HoldStatus {
    /// The name used in storage and the API.
    pub const fn as_str(self) -> &'static str {
        match self {
            HoldStatus::Active => "active",
            HoldStatus::Captured => "captured",
            HoldStatus::Released => "released",
            HoldStatus::Expired => "expired",
        }
    }

    /// The inverse of [`HoldStatus::as_str`].
    pub fn from_name(name: &str) -> Option<HoldStatus> {
        [
            HoldStatus::Active,
            HoldStatus::Captured,
            HoldStatus::Released,
            HoldStatus::Expired,
        ]
        .into_iter()
        .find(|s| s.as_str() == name)
    }
}

impl fmt::Display for HoldStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A hold and everything that happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    id: HoldId,
    account: AccountId,
    held_account: AccountId,
    amount: PositiveAmount,
    captured: i64,
    status: HoldStatus,
    expires_at: Timestamp,
    place_entry: EntryId,
    close_entry: Option<EntryId>,
}

impl Hold {
    /// A hold as stored, for rebuilding it from a database row.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        id: HoldId,
        account: AccountId,
        held_account: AccountId,
        amount: PositiveAmount,
        captured: i64,
        status: HoldStatus,
        expires_at: Timestamp,
        place_entry: EntryId,
        close_entry: Option<EntryId>,
    ) -> Hold {
        Hold {
            id,
            account,
            held_account,
            amount,
            captured,
            status,
            expires_at,
            place_entry,
            close_entry,
        }
    }

    pub const fn id(&self) -> HoldId {
        self.id
    }

    /// The account the money was reserved from, and returns to.
    pub const fn account(&self) -> AccountId {
        self.account
    }

    /// The hold's own account, where the reserved money sits.
    pub const fn held_account(&self) -> AccountId {
        self.held_account
    }

    /// The amount reserved.
    pub const fn amount(&self) -> PositiveAmount {
        self.amount
    }

    /// How much was captured: 0 unless the hold was captured.
    pub const fn captured(&self) -> i64 {
        self.captured
    }

    pub const fn status(&self) -> HoldStatus {
        self.status
    }

    pub const fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// The entry that placed the hold.
    pub const fn place_entry(&self) -> EntryId {
        self.place_entry
    }

    /// The entry that captured, released or expired it, once it has ended.
    pub const fn close_entry(&self) -> Option<EntryId> {
        self.close_entry
    }

    /// What the held account should hold: the amount while the hold is
    /// active, zero once it has ended (INV-18).
    pub const fn held_balance(&self) -> i64 {
        match self.status {
            HoldStatus::Active => self.amount.amount(),
            _ => 0,
        }
    }

    pub(crate) fn close(&mut self, status: HoldStatus, captured: i64, entry: EntryId) {
        self.status = status;
        self.captured = captured;
        self.close_entry = Some(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names_round_trip() {
        for status in [
            HoldStatus::Active,
            HoldStatus::Captured,
            HoldStatus::Released,
            HoldStatus::Expired,
        ] {
            assert_eq!(HoldStatus::from_name(status.as_str()), Some(status));
            assert_eq!(status.to_string(), status.as_str());
        }
        assert_eq!(HoldStatus::from_name("open"), None);
    }
}

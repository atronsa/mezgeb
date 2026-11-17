//! How the simulated client books the offline entries (E16–E18, E25–E31).
//!
//! The engine asks its caller to draft each offline entry and checks that
//! the draft moves exactly what it decided. The reference model drafts the
//! same entries with the same functions, so an entry in the model and its
//! counterpart in PostgreSQL have the same lines, metadata and event time.

use std::collections::BTreeMap;

use mezgeb_core::{Account, AccountId, EntryDraft, Money, PositiveAmount, VoucherId};
use mezgeb_engine::{OfflineBooks, PayRequest, RepayRequest, ReturnRequest, SettleRequest};

fn named(mut draft: EntryDraft, pairs: &[(&str, String)]) -> EntryDraft {
    for (k, v) in pairs {
        draft.metadata.insert((*k).into(), v.clone());
    }
    draft
}

/// E16: the voucher's amount from the wallet into its escrow.
pub fn issue(
    voucher: VoucherId,
    wallet: AccountId,
    escrow: AccountId,
    amount: Money,
) -> EntryDraft {
    let draft = EntryDraft::default()
        .debit(wallet, amount)
        .credit(escrow, amount);
    named(draft, &[("voucher", voucher.to_string())])
}

/// E17.
pub fn settle(r: &SettleRequest) -> EntryDraft {
    let net = Money::from_minor(r.amount.amount() - r.fee.amount(), r.amount.currency());
    let mut draft = EntryDraft::default()
        .debit(r.escrow, r.amount)
        .credit(r.payee, net);
    if let Some(fees) = r.fee_account {
        draft = draft.credit(fees, r.fee);
    }
    draft.event_time = r.event_time;
    named(
        draft,
        &[
            ("voucher", r.voucher.to_string()),
            ("device", r.device.to_string()),
            ("counter", r.counter.to_string()),
        ],
    )
}

/// E26, E28, E29, E31.
pub fn pay(r: &PayRequest) -> EntryDraft {
    let mut draft = EntryDraft::default();
    let mut total = 0;
    for (account, part) in [
        (r.escrow, r.from_escrow),
        (r.wallet, r.from_wallet),
        (r.loss_account, r.from_loss),
    ] {
        if let Some(account) = account {
            draft = draft.debit(account, part);
            total += part.amount();
        }
    }
    let currency = r.fee.currency();
    draft = draft.credit(r.payee, Money::from_minor(total - r.fee.amount(), currency));
    if let Some(fees) = r.fee_account {
        draft = draft.credit(fees, r.fee);
    }
    draft.event_time = r.event_time;
    named(
        draft,
        &[
            ("voucher", r.voucher.to_string()),
            ("device", r.device.to_string()),
            ("counter", r.counter.to_string()),
        ],
    )
}

/// E18, E25, E27.
pub fn return_escrow(r: &ReturnRequest) -> EntryDraft {
    let draft = EntryDraft::default()
        .debit(r.escrow, r.amount)
        .credit(r.wallet, r.amount);
    named(
        draft,
        &[
            ("voucher", r.voucher.to_string()),
            ("reason", r.reason.to_owned()),
        ],
    )
}

/// E30.
pub fn repay(r: &RepayRequest) -> EntryDraft {
    let draft = EntryDraft::default()
        .debit(r.wallet, r.amount)
        .credit(r.loss_account, r.amount);
    named(draft, &[("shortfall", r.shortfall.to_string())])
}

/// The same drafts, for the engine.
pub struct SimBooks;

impl OfflineBooks for SimBooks {
    fn settle(
        &self,
        _: &BTreeMap<AccountId, Account>,
        r: &SettleRequest,
    ) -> Result<EntryDraft, String> {
        Ok(settle(r))
    }

    fn pay(&self, _: &BTreeMap<AccountId, Account>, r: &PayRequest) -> Result<EntryDraft, String> {
        Ok(pay(r))
    }

    fn return_escrow(
        &self,
        _: &BTreeMap<AccountId, Account>,
        r: &ReturnRequest,
    ) -> Result<EntryDraft, String> {
        Ok(return_escrow(r))
    }

    fn repay(
        &self,
        _: &BTreeMap<AccountId, Account>,
        r: &RepayRequest,
    ) -> Result<EntryDraft, String> {
        Ok(repay(r))
    }
}

/// E16 for the engine: `issue_offline_voucher` takes a function.
pub fn issue_book(
    voucher: VoucherId,
    amount: PositiveAmount,
) -> impl Fn(&BTreeMap<AccountId, Account>, AccountId, AccountId) -> Result<EntryDraft, String> {
    move |_, wallet, escrow| Ok(issue(voucher, wallet, escrow, amount.money()))
}

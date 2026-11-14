//! Withdrawals (step 2.02, `docs/accounting/clearing-and-suspense.md`).
//!
//! ```text
//! requested ──► pending (2020) ──sent──► in transit (9030) ──confirmed──► paid (1010)
//!                                                  └──────rejected──────► wallet (2010)
//! ```
//!
//! | Template | Lines | Worked entry |
//! |---|---|---|
//! | `withdrawal_request.v1` | Dr `customer_wallet` (2010) / Cr `pending_withdrawal` (2020, new) | E02 |
//! | `withdrawal_sent.v1` | Dr `pending_withdrawal` (2020) / Cr `withdrawal_in_transit` (9030, new), the whole balance | E03 (a) |
//! | `withdrawal_confirmed.v1` | Dr `withdrawal_in_transit` (9030) / Cr `customer_funds` (1010), the whole balance | E03 (b) |
//! | `withdrawal_failed.v1` | Dr `withdrawal_in_transit` (9030) / Cr `customer_wallet` (2010), the whole balance | E04 (c) |
//!
//! Each withdrawal has its own `2020:wd-…` and `9030:wdt-…` sub-accounts, and
//! neither may go below zero, so each withdrawal has exactly one outcome: a
//! second confirmation, a confirmation after a rejection or a rejection after
//! a confirmation is refused, by this crate (2014) and again by the posting
//! rules (1012). Whether a customer may withdraw (limits, KYC) is the
//! client's decision; whether they have the money is checked by the posting
//! rules (INV-8).

use mezgeb_core::PositiveAmount;

use crate::deposits::{AmountRequest, amount_request};
use crate::error::TemplateError;
use crate::template::{Accounts, Input, Leg, Role, Template};

/// 1010 Bank: customer funds account.
const CUSTOMER_FUNDS: &[u16] = &[1010];
/// 2010 Customer wallets.
const CUSTOMER_WALLETS: &[u16] = &[2010];
/// 2020 Pending withdrawals.
const PENDING_WITHDRAWALS: &[u16] = &[2020];
/// 9030 Clearing: withdrawals in transit.
const IN_TRANSIT: &[u16] = &[9030];

/// Moves a whole item from `from` to `to`.
fn move_whole(
    from: &'static str,
    to: &'static str,
    amount: PositiveAmount,
    accounts: &Accounts<'_>,
) -> Result<Vec<Leg>, TemplateError> {
    accounts.require_whole_balance(from, amount)?;
    Ok(vec![Leg::debit(from, amount), Leg::credit(to, amount)])
}

/// A customer asks for a payout: the money stops being spendable and opens
/// a new pending withdrawal (E02).
#[derive(Debug, Clone, Copy)]
pub struct WithdrawalRequestV1;

impl Template for WithdrawalRequestV1 {
    const NAME: &'static str = "withdrawal_request";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("customer_wallet", CUSTOMER_WALLETS),
        Role::new("pending_withdrawal", PENDING_WITHDRAWALS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_new("pending_withdrawal")?;
        Ok(vec![
            Leg::debit("customer_wallet", request.amount),
            Leg::credit("pending_withdrawal", request.amount),
        ])
    }
}

/// The payout is submitted to the bank: the whole pending withdrawal moves
/// into its own in-transit item (E03 (a)).
#[derive(Debug, Clone, Copy)]
pub struct WithdrawalSentV1;

impl Template for WithdrawalSentV1 {
    const NAME: &'static str = "withdrawal_sent";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("pending_withdrawal", PENDING_WITHDRAWALS),
        Role::new("withdrawal_in_transit", IN_TRANSIT),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_new("withdrawal_in_transit")?;
        move_whole(
            "pending_withdrawal",
            "withdrawal_in_transit",
            request.amount,
            accounts,
        )
    }
}

/// The bank confirms the payout: the cash leaves 1010 (E03 (b)).
#[derive(Debug, Clone, Copy)]
pub struct WithdrawalConfirmedV1;

impl Template for WithdrawalConfirmedV1 {
    const NAME: &'static str = "withdrawal_confirmed";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("withdrawal_in_transit", IN_TRANSIT),
        Role::new("customer_funds", CUSTOMER_FUNDS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        move_whole(
            "withdrawal_in_transit",
            "customer_funds",
            request.amount,
            accounts,
        )
    }
}

/// The bank rejects the payout: exactly the amount in transit goes back to
/// the wallet (E04 (c)).
#[derive(Debug, Clone, Copy)]
pub struct WithdrawalFailedV1;

impl Template for WithdrawalFailedV1 {
    const NAME: &'static str = "withdrawal_failed";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("withdrawal_in_transit", IN_TRANSIT),
        Role::new("customer_wallet", CUSTOMER_WALLETS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        move_whole(
            "withdrawal_in_transit",
            "customer_wallet",
            request.amount,
            accounts,
        )
    }
}

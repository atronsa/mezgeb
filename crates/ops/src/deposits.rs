//! Deposits and suspense (step 2.02, `docs/accounting/clearing-and-suspense.md`).
//!
//! | Template | Lines | Worked entry |
//! |---|---|---|
//! | `deposit_confirmed.v1` | Dr `customer_funds` (1010) / Cr `customer_wallet` (2010) | E01, E05 |
//! | `deposit_unmatched.v1` | Dr `customer_funds` (1010) / Cr `suspense_item` (9010, new) | E19 |
//! | `suspense_resolve.v1` | Dr `suspense_item` (9010) / Cr `owner` (2010 or 2030), the whole balance | E20 |
//!
//! Account codes are from the wallet-provider chart
//! (`charts/wallet-provider.toml`). A deposit is booked when the bank
//! statement shows it, never on an earlier notice, so 1010 always equals the
//! bank.

use mezgeb_core::PositiveAmount;

use crate::error::TemplateError;
use crate::template::{Accounts, Input, Leg, Role, Template};

/// 1010 Bank: customer funds account.
const CUSTOMER_FUNDS: &[u16] = &[1010];
/// 2010 Customer wallets.
const CUSTOMER_WALLETS: &[u16] = &[2010];
/// 9010 Suspense: unidentified funds.
const SUSPENSE: &[u16] = &[9010];
/// Who a suspense item can belong to: a customer (2010) or a merchant (2030).
const OWNERS: &[u16] = &[2010, 2030];

/// The typed request of a template that reads one amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmountRequest {
    pub amount: PositiveAmount,
}

pub(crate) fn amount_request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
    Ok(AmountRequest {
        amount: input.amount("amount")?,
    })
}

/// Money from a known customer has arrived in the bank (E01). Also books a
/// payout the bank returned after confirming it (E05).
#[derive(Debug, Clone, Copy)]
pub struct DepositConfirmedV1;

impl Template for DepositConfirmedV1 {
    const NAME: &'static str = "deposit_confirmed";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("customer_funds", CUSTOMER_FUNDS),
        Role::new("customer_wallet", CUSTOMER_WALLETS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        Ok(vec![
            Leg::debit("customer_funds", request.amount),
            Leg::credit("customer_wallet", request.amount),
        ])
    }
}

/// Money has arrived in the bank, but its owner is not known: it opens a new
/// suspense item (E19).
#[derive(Debug, Clone, Copy)]
pub struct DepositUnmatchedV1;

impl Template for DepositUnmatchedV1 {
    const NAME: &'static str = "deposit_unmatched";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("customer_funds", CUSTOMER_FUNDS),
        Role::new("suspense_item", SUSPENSE),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_new("suspense_item")?;
        Ok(vec![
            Leg::debit("customer_funds", request.amount),
            Leg::credit("suspense_item", request.amount),
        ])
    }
}

/// The owner of a suspense item is found: the whole item moves to their
/// wallet or merchant balance (E20).
#[derive(Debug, Clone, Copy)]
pub struct SuspenseResolveV1;

impl Template for SuspenseResolveV1 {
    const NAME: &'static str = "suspense_resolve";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("suspense_item", SUSPENSE),
        Role::new("owner", OWNERS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_whole_balance("suspense_item", request.amount)?;
        Ok(vec![
            Leg::debit("suspense_item", request.amount),
            Leg::credit("owner", request.amount),
        ])
    }
}

//! Reconciliation suspense (step 2.15, ADR-0020,
//! `docs/accounting/entries/reconciliation.md`).
//!
//! | Template | Lines | Worked entry |
//! |---|---|---|
//! | `bank_debit_unmatched.v1` | Dr `bank_debit_item` (9040, new) / Cr `customer_funds` (1010) | E32, E34 |
//! | `bank_debit_resolve.v1` | Dr `cause` (1010, 2010, 2030 or 5020) / Cr `bank_debit_item` (9040), the whole balance | E33, E35 |
//!
//! When the bank shows less than the ledger, the first template makes 1010
//! equal the bank again and parks the difference in its own 9040 item; the
//! second closes the item once its cause is known. When the bank shows more,
//! `deposit_unmatched.v1` and `suspense_resolve.v1` do the same through 9010.

use crate::deposits::{AmountRequest, amount_request};
use crate::error::TemplateError;
use crate::template::{Accounts, Input, Leg, Role, Template};

/// 1010 Bank: customer funds account.
const CUSTOMER_FUNDS: &[u16] = &[1010];
/// 9040 Suspense: unexplained bank debits.
const BANK_DEBITS: &[u16] = &[9040];
/// What an unexplained bank debit can turn out to be: money the bank gives
/// back (1010), money a customer (2010) or a merchant (2030) was credited
/// but never sent, or a bank charge (5020).
const CAUSES: &[u16] = &[1010, 2010, 2030, 5020];

/// The bank shows less in 1010 than the ledger: the difference moves to a
/// new suspense item (E32, E34).
#[derive(Debug, Clone, Copy)]
pub struct BankDebitUnmatchedV1;

impl Template for BankDebitUnmatchedV1 {
    const NAME: &'static str = "bank_debit_unmatched";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("bank_debit_item", BANK_DEBITS),
        Role::new("customer_funds", CUSTOMER_FUNDS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_new("bank_debit_item")?;
        Ok(vec![
            Leg::debit("bank_debit_item", request.amount),
            Leg::credit("customer_funds", request.amount),
        ])
    }
}

/// The cause of an unexplained bank debit is found: the whole item closes
/// against it (E33, E35).
#[derive(Debug, Clone, Copy)]
pub struct BankDebitResolveV1;

impl Template for BankDebitResolveV1 {
    const NAME: &'static str = "bank_debit_resolve";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("cause", CAUSES),
        Role::new("bank_debit_item", BANK_DEBITS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_whole_balance("bank_debit_item", request.amount)?;
        Ok(vec![
            Leg::debit("cause", request.amount),
            Leg::credit("bank_debit_item", request.amount),
        ])
    }
}

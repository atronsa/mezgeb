//! `issue_voucher.v1`: reserve an offline allowance in its own escrow
//! (worked entry E16, `docs/accounting/entries/offline-escrow.md`;
//! ADR-0019).
//!
//! | Role | Account | Dr | Cr |
//! |---|---|---:|---:|
//! | `customer_wallet` | 2010 Customer wallets | amount | |
//! | `voucher_escrow` | 2050 Offline escrow, new | | amount |
//!
//! Each voucher has its own `2050:vch-…` sub-account, so it must be new: an
//! escrow is never shared or refilled. The entry records the voucher it
//! funds under `voucher`. Whether the device may get a voucher (its status,
//! its tier's limit, the customer's cap) is checked by the engine before it
//! books this; whether the wallet has the money is checked by the posting
//! rules (INV-8).

use crate::deposits::{AmountRequest, amount_request};
use crate::error::TemplateError;
use crate::template::{Accounts, Input, Leg, MetadataField, Role, Template};

/// 2010 Customer wallets.
const CUSTOMER_WALLETS: &[u16] = &[2010];
/// 2050 Offline escrow.
const OFFLINE_ESCROW: &[u16] = &[2050];

/// Version 1 of the voucher issue.
#[derive(Debug, Clone, Copy)]
pub struct IssueVoucherV1;

impl Template for IssueVoucherV1 {
    const NAME: &'static str = "issue_voucher";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("customer_wallet", CUSTOMER_WALLETS),
        Role::new("voucher_escrow", OFFLINE_ESCROW),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const METADATA: &'static [MetadataField] = &[MetadataField::required("voucher", &[])];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_new("voucher_escrow")?;
        Ok(vec![
            Leg::debit("customer_wallet", request.amount),
            Leg::credit("voucher_escrow", request.amount),
        ])
    }
}

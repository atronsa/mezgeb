//! Offline payments (ADR-0019, `docs/accounting/entries/offline-escrow.md`).
//! Both templates are engine-only: the engine books them as one step of
//! issuing a voucher or settling a receipt.
//!
//! `issue_voucher.v1` reserves an offline allowance in its own escrow
//! (worked entry E16):
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
use crate::payments::{FEE_REVENUE, PAYEES, PaymentRequest, book_payment};
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

/// `offline_receipt_settle.v1`: pays an offline receipt from its voucher's
/// escrow (worked entry E17).
///
/// | Role | Account | Dr | Cr |
/// |---|---|---:|---:|
/// | `voucher_escrow` | 2050 Offline escrow | amount | |
/// | `payee` | 2010 Customer wallets or 2030 Merchant balances | | amount − merchant fee |
/// | `fee_revenue` | 4010 Payment fee revenue | | merchant fee |
///
/// It is the merchant payment of E07, from escrow instead of the wallet:
/// offline is another way to authorise a payment, not another kind. The
/// entry names the voucher, the device and the counter it settles. Whether
/// the receipt may settle (signature, order, conflicts, the escrow left) is
/// the engine's decision; the posting rules keep the escrow from going
/// below zero (INV-8, INV-19).
#[derive(Debug, Clone, Copy)]
pub struct OfflineReceiptSettleV1;

impl Template for OfflineReceiptSettleV1 {
    const NAME: &'static str = "offline_receipt_settle";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("voucher_escrow", OFFLINE_ESCROW),
        Role::new("payee", PAYEES),
        Role::optional("fee_revenue", FEE_REVENUE),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const OPTIONAL_AMOUNTS: &'static [&'static str] = &["merchant_fee"];
    const METADATA: &'static [MetadataField] = &[
        MetadataField::required("voucher", &[]),
        MetadataField::required("device", &[]),
        MetadataField::required("counter", &[]),
    ];
    type Request = PaymentRequest;

    fn request(input: &Input<'_>) -> Result<PaymentRequest, TemplateError> {
        Ok(PaymentRequest {
            amount: input.amount("amount")?,
            merchant_fee: input.optional_amount("merchant_fee")?,
            payer_fee: None,
        })
    }

    fn book(request: &PaymentRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        book_payment("voucher_escrow", "payee", request)
    }
}

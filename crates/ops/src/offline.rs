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

use mezgeb_core::{Money, PositiveAmount};

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

/// 2010 Customer wallets, for the payer of a late receipt.
const PAYER_WALLETS: &[u16] = &[2010];
/// 5030 Offline settlement losses.
const OFFLINE_LOSS: &[u16] = &[5030];

/// The reasons a voucher's escrow goes back to its wallet.
pub const RETURN_REASONS: &[&str] = &["expired", "closed", "dispute"];

/// `offline_voucher_return.v1`: the whole escrow left goes back to the
/// payer's wallet (worked entries E18, E25, E27).
///
/// | Role | Account | Dr | Cr |
/// |---|---|---:|---:|
/// | `voucher_escrow` | 2050 Offline escrow | amount | |
/// | `customer_wallet` | 2010 Customer wallets | | amount |
///
/// The amount must be the escrow's whole balance (2014), so the escrow ends
/// at zero (INV-23). The `reason` is `expired` (after expiry and grace),
/// `closed` (an early close) or `dispute` (when a frozen voucher's case
/// ends).
#[derive(Debug, Clone, Copy)]
pub struct OfflineVoucherReturnV1;

impl Template for OfflineVoucherReturnV1 {
    const NAME: &'static str = "offline_voucher_return";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("voucher_escrow", OFFLINE_ESCROW),
        Role::new("customer_wallet", CUSTOMER_WALLETS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const METADATA: &'static [MetadataField] = &[
        MetadataField::required("voucher", &[]),
        MetadataField::required("reason", RETURN_REASONS),
    ];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_whole_balance("voucher_escrow", request.amount)?;
        Ok(vec![
            Leg::debit("voucher_escrow", request.amount),
            Leg::credit("customer_wallet", request.amount),
        ])
    }
}

/// The typed request of [`OfflineReceiptPayV1`]: where the money comes
/// from, and the merchant fee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptPayRequest {
    pub from_escrow: Option<PositiveAmount>,
    pub from_wallet: Option<PositiveAmount>,
    pub from_loss: Option<PositiveAmount>,
    pub merchant_fee: Option<PositiveAmount>,
}

/// `offline_receipt_pay.v1`: pays an offline receipt that escrow alone
/// does not cover, from escrow, then the payer's wallet, then the offline
/// loss account (ADR-0019 loss policy; worked entries E26, E28, E29, E31).
///
/// | Role | Account | Dr | Cr |
/// |---|---|---:|---:|
/// | `voucher_escrow` (optional) | 2050 Offline escrow | `from_escrow` | |
/// | `payer_wallet` (optional) | 2010 Customer wallets | `from_wallet` | |
/// | `offline_loss` (optional) | 5030 Offline settlement losses | `from_loss` | |
/// | `payee` | 2010 or 2030 | | total − merchant fee |
/// | `fee_revenue` (optional) | 4010 Payment fee revenue | | merchant fee |
///
/// The receipt's amount is the sum of the parts given; at least one is
/// (2004). The engine decides the parts: escrow as far as it goes, the
/// wallet as far as its balance allows (it cannot go negative, INV-8), the
/// loss account for the rest, which it then records against the payer.
#[derive(Debug, Clone, Copy)]
pub struct OfflineReceiptPayV1;

impl Template for OfflineReceiptPayV1 {
    const NAME: &'static str = "offline_receipt_pay";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::optional("voucher_escrow", OFFLINE_ESCROW),
        Role::optional("payer_wallet", PAYER_WALLETS),
        Role::optional("offline_loss", OFFLINE_LOSS),
        Role::new("payee", PAYEES),
        Role::optional("fee_revenue", FEE_REVENUE),
    ];
    const AMOUNTS: &'static [&'static str] = &[];
    const OPTIONAL_AMOUNTS: &'static [&'static str] =
        &["from_escrow", "from_wallet", "from_loss", "merchant_fee"];
    const METADATA: &'static [MetadataField] = &[
        MetadataField::required("voucher", &[]),
        MetadataField::required("device", &[]),
        MetadataField::required("counter", &[]),
    ];
    type Request = ReceiptPayRequest;

    fn request(input: &Input<'_>) -> Result<ReceiptPayRequest, TemplateError> {
        Ok(ReceiptPayRequest {
            from_escrow: input.optional_amount("from_escrow")?,
            from_wallet: input.optional_amount("from_wallet")?,
            from_loss: input.optional_amount("from_loss")?,
            merchant_fee: input.optional_amount("merchant_fee")?,
        })
    }

    fn book(request: &ReceiptPayRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        let parts = [
            ("voucher_escrow", "from_escrow", request.from_escrow),
            ("payer_wallet", "from_wallet", request.from_wallet),
            ("offline_loss", "from_loss", request.from_loss),
        ];
        let Some(first) = parts.iter().find_map(|(_, _, p)| *p) else {
            return Err(TemplateError::MissingAmount("from_escrow".into()));
        };
        let currency = first.currency();
        let mut total: i64 = 0;
        let mut legs = Vec::with_capacity(5);
        for (role, name, part) in parts {
            let Some(part) = part else { continue };
            if part.currency() != currency {
                return Err(TemplateError::CurrencyMismatch(name.to_owned()));
            }
            total = total
                .checked_add(part.amount())
                .ok_or(TemplateError::AmountOverflow)?;
            legs.push(Leg::debit(role, part));
        }
        if request
            .merchant_fee
            .is_some_and(|f| f.currency() != currency)
        {
            return Err(TemplateError::CurrencyMismatch("merchant_fee".into()));
        }
        let fee = request.merchant_fee.map_or(0, PositiveAmount::amount);
        if fee > total {
            return Err(TemplateError::FeeExceedsAmount);
        }
        let positive = |minor: i64| PositiveAmount::new(Money::from_minor(minor, currency)).ok();
        if let Some(net) = positive(total - fee) {
            legs.push(Leg::credit("payee", net));
        }
        if let Some(fee) = positive(fee) {
            legs.push(Leg::credit("fee_revenue", fee));
        }
        Ok(legs)
    }
}

/// `offline_shortfall_repay.v1`: the payer repays a shortfall the loss
/// account paid for them (worked entry E30).
///
/// | Role | Account | Dr | Cr |
/// |---|---|---:|---:|
/// | `customer_wallet` | 2010 Customer wallets | amount | |
/// | `offline_loss` | 5030 Offline settlement losses | | amount |
///
/// The wallet must hold the amount (INV-8). The entry names the shortfall.
#[derive(Debug, Clone, Copy)]
pub struct OfflineShortfallRepayV1;

impl Template for OfflineShortfallRepayV1 {
    const NAME: &'static str = "offline_shortfall_repay";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("customer_wallet", CUSTOMER_WALLETS),
        Role::new("offline_loss", OFFLINE_LOSS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const METADATA: &'static [MetadataField] = &[MetadataField::required("shortfall", &[])];
    type Request = AmountRequest;

    fn request(input: &Input<'_>) -> Result<AmountRequest, TemplateError> {
        amount_request(input)
    }

    fn book(request: &AmountRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        Ok(vec![
            Leg::debit("customer_wallet", request.amount),
            Leg::credit("offline_loss", request.amount),
        ])
    }
}

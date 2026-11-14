//! Refunds and disputes (step 2.04, ADR-0015,
//! `docs/accounting/refunds-and-disputes.md`).
//!
//! | Template | Lines | Worked entry |
//! |---|---|---|
//! | `merchant_refund.v1` | Dr `merchant_balance` (2030) / Dr `fee_revenue` (4010) / Cr `payer_wallet` (2010) | E11, E12 |
//! | `dispute_open.v1` | Dr `merchant_balance` (2030) / Cr `dispute_item` (2060, new) | E21, E23 |
//! | `dispute_resolve_payer.v1` | Dr `dispute_item` (2060) / Cr `payer_wallet` (2010), the whole item | E22 |
//! | `dispute_resolve_merchant.v1` | Dr `dispute_item` (2060) / Cr `merchant_balance` (2030), the whole item | E24 |
//!
//! `merchant_refund.v1` and `dispute_resolve_payer.v1` are refunds: the request names
//! the payment they refund, and the engine checks them against it (INV-15).
//! Whatever refunds and dispute resolutions are tried, the payer never gets
//! back more than they paid, and a payment that has been reversed cannot be
//! refunded.
//!
//! How much of the fee goes back is the client's decision (ADR-0015): it
//! sends `fee_refund`, the part of the refund that comes out of fee revenue,
//! and the rest comes out of the merchant's balance.
//!
//! A dispute holds the disputed amount in its own `2060:dsp-…` sub-account
//! until it is resolved, wholly for the payer or wholly for the merchant. If
//! the merchant's balance cannot cover the dispute, opening it is refused
//! (the posting rules refuse to take the balance below zero); the policy
//! says what happens then.

use mezgeb_core::{Money, PositiveAmount};

use crate::error::TemplateError;
use crate::template::{Accounts, Input, Leg, MetadataField, Role, Template};

/// 2010 Customer wallets.
const CUSTOMER_WALLETS: &[u16] = &[2010];
/// 2030 Merchant balances.
const MERCHANT_BALANCES: &[u16] = &[2030];
/// 2060 Disputed merchant funds.
const DISPUTES: &[u16] = &[2060];
/// 4010 Payment fee revenue.
const FEE_REVENUE: &[u16] = &[4010];

/// The typed request of `merchant_refund.v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefundRequest {
    /// What the payer gets back.
    pub amount: PositiveAmount,
    /// The part of it that comes out of fee revenue, if any.
    pub fee_refund: Option<PositiveAmount>,
}

/// A merchant gives money back for a payment (E11, E12).
#[derive(Debug, Clone, Copy)]
pub struct MerchantRefundV1;

impl Template for MerchantRefundV1 {
    const NAME: &'static str = "merchant_refund";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("merchant_balance", MERCHANT_BALANCES),
        Role::optional("fee_revenue", FEE_REVENUE),
        Role::new("payer_wallet", CUSTOMER_WALLETS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const OPTIONAL_AMOUNTS: &'static [&'static str] = &["fee_refund"];
    const REFUNDS: bool = true;
    type Request = RefundRequest;

    fn request(input: &Input<'_>) -> Result<RefundRequest, TemplateError> {
        Ok(RefundRequest {
            amount: input.amount("amount")?,
            fee_refund: input.optional_amount("fee_refund")?,
        })
    }

    fn book(request: &RefundRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        let amount = request.amount;
        let fee = match request.fee_refund {
            Some(fee) if fee.currency() != amount.currency() => {
                return Err(TemplateError::CurrencyMismatch("fee_refund".to_owned()));
            }
            Some(fee) if fee.amount() > amount.amount() => {
                return Err(TemplateError::FeeExceedsAmount);
            }
            Some(fee) => fee.amount(),
            None => 0,
        };
        let part =
            |minor: i64| PositiveAmount::new(Money::from_minor(minor, amount.currency())).ok();
        let mut legs = Vec::with_capacity(3);
        if let Some(merchant) = part(amount.amount() - fee) {
            legs.push(Leg::debit("merchant_balance", merchant));
        }
        if let Some(fee) = part(fee) {
            legs.push(Leg::debit("fee_revenue", fee));
        }
        legs.push(Leg::credit("payer_wallet", amount));
        Ok(legs)
    }
}

fn amount(input: &Input<'_>) -> Result<PositiveAmount, TemplateError> {
    input.amount("amount")
}

/// A payer disputes a payment: the disputed amount moves from the
/// merchant's balance into a new dispute item, where neither side can spend
/// it (E21, E23).
#[derive(Debug, Clone, Copy)]
pub struct DisputeOpenV1;

impl Template for DisputeOpenV1 {
    const NAME: &'static str = "dispute_open";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("merchant_balance", MERCHANT_BALANCES),
        Role::new("dispute_item", DISPUTES),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const METADATA: &'static [MetadataField] = &[MetadataField::required("disputed_entry", &[])];
    type Request = PositiveAmount;

    fn request(input: &Input<'_>) -> Result<PositiveAmount, TemplateError> {
        amount(input)
    }

    fn book(amount: &PositiveAmount, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_new("dispute_item")?;
        Ok(vec![
            Leg::debit("merchant_balance", *amount),
            Leg::credit("dispute_item", *amount),
        ])
    }
}

/// The dispute is decided for the payer: the whole item goes to the payer,
/// as a refund of the disputed payment (E22).
#[derive(Debug, Clone, Copy)]
pub struct DisputeResolvePayerV1;

impl Template for DisputeResolvePayerV1 {
    const NAME: &'static str = "dispute_resolve_payer";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("dispute_item", DISPUTES),
        Role::new("payer_wallet", CUSTOMER_WALLETS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const REFUNDS: bool = true;
    type Request = PositiveAmount;

    fn request(input: &Input<'_>) -> Result<PositiveAmount, TemplateError> {
        amount(input)
    }

    fn book(amount: &PositiveAmount, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_whole_balance("dispute_item", *amount)?;
        Ok(vec![
            Leg::debit("dispute_item", *amount),
            Leg::credit("payer_wallet", *amount),
        ])
    }
}

/// The dispute is decided for the merchant: the whole item goes back to the
/// merchant's balance (E24).
#[derive(Debug, Clone, Copy)]
pub struct DisputeResolveMerchantV1;

impl Template for DisputeResolveMerchantV1 {
    const NAME: &'static str = "dispute_resolve_merchant";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("dispute_item", DISPUTES),
        Role::new("merchant_balance", MERCHANT_BALANCES),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = PositiveAmount;

    fn request(input: &Input<'_>) -> Result<PositiveAmount, TemplateError> {
        amount(input)
    }

    fn book(amount: &PositiveAmount, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        accounts.require_whole_balance("dispute_item", *amount)?;
        Ok(vec![
            Leg::debit("dispute_item", *amount),
            Leg::credit("merchant_balance", *amount),
        ])
    }
}

//! Merchant payments and payment requests, with fees in the same entry
//! (step 2.03, ADR-0014).
//!
//! | Template | Lines | Worked entry |
//! |---|---|---|
//! | `merchant_payment.v1` | Dr `payer_wallet` (2010) / Cr `merchant_balance` (2030) / Cr `fee_revenue` (4010) | E07 (QR), E08 (card tap), E09 (fingerprint) |
//! | `payment_request_settle.v1` | Dr `payer_wallet` (2010) / Cr `payee` (2010 or 2030) / Cr `fee_revenue` (4010) | E10 |
//!
//! Both read an `amount`, the price, and two optional fees, already computed
//! by the client (the ledger never rounds, ADR-0002):
//!
//! - `merchant_fee`, paid by the payee out of the price: the payee is
//!   credited `amount − merchant_fee` (E07–E09);
//! - `payer_fee`, paid by the payer on top of the price: the payer is
//!   debited `amount + payer_fee`.
//!
//! Fee revenue is credited with both fees together, in the same entry as the
//! payment, so the payment and its fee post or fail together. A zero or
//! absent fee books no fee line, and then `fee_revenue` need not be bound.
//! The fees together may not exceed the amount (2016), and every amount is
//! in one currency (2017).
//!
//! `merchant_payment.v1` requires the `channel` the customer used, one of
//! [`CHANNELS`]; the channel is metadata, never a different account
//! (ADR-0014). `payment_request_settle.v1` requires the `payment_request`
//! it settles.

use mezgeb_core::{Money, PositiveAmount};

use crate::error::TemplateError;
use crate::template::{Accounts, Input, Leg, MetadataField, Role, Template};

/// The channels a merchant payment can be made through (ADR-0014).
pub const CHANNELS: &[&str] = &["qr", "card_tap", "phone_tap", "fingerprint"];

/// 2010 Customer wallets.
const CUSTOMER_WALLETS: &[u16] = &[2010];
/// 2030 Merchant balances.
const MERCHANT_BALANCES: &[u16] = &[2030];
/// Who a payment request can be paid to: a customer or a merchant.
const PAYEES: &[u16] = &[2010, 2030];
/// 4010 Payment fee revenue.
const FEE_REVENUE: &[u16] = &[4010];

/// The typed request of both payment templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaymentRequest {
    /// The price.
    pub amount: PositiveAmount,
    /// The fee the payee pays out of the price, if any.
    pub merchant_fee: Option<PositiveAmount>,
    /// The fee the payer pays on top of the price, if any.
    pub payer_fee: Option<PositiveAmount>,
}

fn payment_request(input: &Input<'_>) -> Result<PaymentRequest, TemplateError> {
    Ok(PaymentRequest {
        amount: input.amount("amount")?,
        merchant_fee: input.optional_amount("merchant_fee")?,
        payer_fee: input.optional_amount("payer_fee")?,
    })
}

fn positive(minor: i64, like: PositiveAmount) -> Option<PositiveAmount> {
    PositiveAmount::new(Money::from_minor(minor, like.currency())).ok()
}

/// The lines of a payment from `payer` to `payee`, with its fees.
fn book_payment(
    payer: &'static str,
    payee: &'static str,
    request: &PaymentRequest,
) -> Result<Vec<Leg>, TemplateError> {
    let amount = request.amount;
    for (name, fee) in [
        ("merchant_fee", request.merchant_fee),
        ("payer_fee", request.payer_fee),
    ] {
        if fee.is_some_and(|f| f.currency() != amount.currency()) {
            return Err(TemplateError::CurrencyMismatch(name.to_owned()));
        }
    }
    let merchant_fee = request.merchant_fee.map_or(0, PositiveAmount::amount);
    let payer_fee = request.payer_fee.map_or(0, PositiveAmount::amount);
    let fees = merchant_fee
        .checked_add(payer_fee)
        .ok_or(TemplateError::AmountOverflow)?;
    if fees > amount.amount() {
        return Err(TemplateError::FeeExceedsAmount);
    }
    let charged = amount
        .amount()
        .checked_add(payer_fee)
        .ok_or(TemplateError::AmountOverflow)?;

    let mut legs = Vec::with_capacity(3);
    if let Some(charged) = positive(charged, amount) {
        legs.push(Leg::debit(payer, charged));
    }
    if let Some(net) = positive(amount.amount() - merchant_fee, amount) {
        legs.push(Leg::credit(payee, net));
    }
    if let Some(fees) = positive(fees, amount) {
        legs.push(Leg::credit("fee_revenue", fees));
    }
    Ok(legs)
}

/// A customer pays a merchant, through any channel (E07–E09).
#[derive(Debug, Clone, Copy)]
pub struct MerchantPaymentV1;

impl Template for MerchantPaymentV1 {
    const NAME: &'static str = "merchant_payment";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("payer_wallet", CUSTOMER_WALLETS),
        Role::new("merchant_balance", MERCHANT_BALANCES),
        Role::optional("fee_revenue", FEE_REVENUE),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const OPTIONAL_AMOUNTS: &'static [&'static str] = &["merchant_fee", "payer_fee"];
    const METADATA: &'static [MetadataField] = &[MetadataField::required("channel", CHANNELS)];
    type Request = PaymentRequest;

    fn request(input: &Input<'_>) -> Result<PaymentRequest, TemplateError> {
        payment_request(input)
    }

    fn book(request: &PaymentRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        book_payment("payer_wallet", "merchant_balance", request)
    }
}

/// A customer pays a payment request from another customer or a merchant
/// (E10). The request itself is not booked: it is a message until it is
/// paid.
#[derive(Debug, Clone, Copy)]
pub struct PaymentRequestSettleV1;

impl Template for PaymentRequestSettleV1 {
    const NAME: &'static str = "payment_request_settle";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("payer_wallet", CUSTOMER_WALLETS),
        Role::new("payee", PAYEES),
        Role::optional("fee_revenue", FEE_REVENUE),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    const OPTIONAL_AMOUNTS: &'static [&'static str] = &["merchant_fee", "payer_fee"];
    const METADATA: &'static [MetadataField] = &[MetadataField::required("payment_request", &[])];
    type Request = PaymentRequest;

    fn request(input: &Input<'_>) -> Result<PaymentRequest, TemplateError> {
        payment_request(input)
    }

    fn book(request: &PaymentRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        book_payment("payer_wallet", "payee", request)
    }
}

//! The over-refund property (step 2.04, INV-15): whatever refunds and
//! reversals are tried against a payment, in any order and any amounts, the
//! money given back never exceeds what was paid, per account, and a refund
//! and a reversal of the same payment never both succeed.

mod common;

use common::CHART;
use mezgeb_core::{
    AccountId, ActorId, Authorization, Chart, Currency, EntryDraft, EntryId, LedgerError,
    MemLedger, Money, ReasonCode, check_invariants,
};
use proptest::prelude::*;

fn id(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

#[derive(Debug, Clone)]
enum Attempt {
    /// Give back `merchant` from the merchant and `fee` from fee revenue.
    Refund {
        merchant: i64,
        fee: i64,
    },
    /// Give money back to someone who never paid.
    RefundToSomeoneElse {
        amount: i64,
    },
    Reverse,
}

fn attempt(price: i64) -> impl Strategy<Value = Attempt> {
    prop_oneof![
        6 => (0..=price, 0..=price / 5).prop_map(|(merchant, fee)| Attempt::Refund { merchant, fee }),
        // Small refunds, so that sequences reach the limit exactly.
        6 => (0..=price / 4, 0..=price / 50)
            .prop_map(|(merchant, fee)| Attempt::Refund { merchant, fee }),
        1 => (1..=price).prop_map(|amount| Attempt::RefundToSomeoneElse { amount }),
        1 => Just(Attempt::Reverse),
    ]
}

fn case() -> impl Strategy<Value = (i64, i64, Vec<Attempt>)> {
    (1i64..=1_000_000).prop_flat_map(|price| {
        (
            Just(price),
            0..=price / 10,
            prop::collection::vec(attempt(price), 1..12),
        )
    })
}

struct Accounts {
    payer: AccountId,
    other: AccountId,
    merchant: AccountId,
    fees: AccountId,
}

fn setup(
    price: i64,
    fee: i64,
) -> Result<(MemLedger, Accounts, EntryId), Box<dyn std::error::Error>> {
    // Parsed once: parsing it for each of the 10,000 cases is most of the
    // run time.
    static PARSED: std::sync::OnceLock<Result<Chart, String>> = std::sync::OnceLock::new();
    let chart = PARSED
        .get_or_init(|| Chart::from_toml(CHART).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| e.clone())?;
    let mut ledger = MemLedger::new();
    let bank: AccountId = id(1).parse()?;
    let accounts = Accounts {
        payer: id(2).parse()?,
        other: id(3).parse()?,
        merchant: id(4).parse()?,
        fees: id(5).parse()?,
    };
    ledger.open_account(bank, chart, "1010")?;
    ledger.open_account(accounts.payer, chart, "2010:cust-abebe")?;
    ledger.open_account(accounts.other, chart, "2010:cust-hana")?;
    ledger.open_account(accounts.merchant, chart, "2030:merch-coffee")?;
    ledger.open_account(accounts.fees, chart, "4010")?;
    ledger.post(
        id(100).parse()?,
        EntryDraft::default()
            .debit(bank, etb(price))
            .credit(accounts.payer, etb(price)),
    )?;
    let payment: EntryId = id(101).parse()?;
    let mut draft = EntryDraft::default()
        .debit(accounts.payer, etb(price))
        .credit(accounts.merchant, etb(price - fee));
    if fee > 0 {
        draft = draft.credit(accounts.fees, etb(fee));
    }
    ledger.post(payment, draft)?;
    Ok((ledger, accounts, payment))
}

fn balance(ledger: &MemLedger, account: AccountId) -> i64 {
    ledger.account(account).map_or(0, |a| a.balance().amount())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    /// The acceptance criterion of step 2.04, over 10,000 runs.
    #[test]
    fn refunds_never_give_back_more_than_was_paid((price, fee, attempts) in case()) {
        let (mut ledger, a, payment) =
            setup(price, fee).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let net = price - fee;
        // What is left to give back, per account, as the model sees it.
        let (mut left_payer, mut left_merchant, mut left_fee) = (price, net, fee);
        // With no fee, fee revenue is not part of the payment: the rule lets
        // a refund take money from it, and the posting rules refuse because
        // it holds nothing.
        let fee_in_payment = fee > 0;
        let mut reversed = false;
        let mut refunded = false;

        for (n, attempt) in attempts.iter().enumerate() {
            let entry: EntryId = id(1_000 + n as u64)
                .parse()
                .map_err(|e: mezgeb_core::IdError| TestCaseError::fail(e.to_string()))?;
            let before = ledger.entries().len();
            match attempt {
                Attempt::Refund { merchant, fee: back } => {
                    if merchant + back == 0 {
                        continue;
                    }
                    let mut draft = EntryDraft::default();
                    if *merchant > 0 {
                        draft = draft.debit(a.merchant, etb(*merchant));
                    }
                    if *back > 0 {
                        draft = draft.debit(a.fees, etb(*back));
                    }
                    draft = draft.credit(a.payer, etb(merchant + back));
                    draft.refunds = Some(payment);
                    let fits = *merchant <= left_merchant
                        && (!fee_in_payment || *back <= left_fee)
                        && merchant + back <= left_payer;
                    let fee_revenue_empty = !fee_in_payment && *back > 0;
                    match ledger.post(entry, draft) {
                        Ok(_) => {
                            prop_assert!(
                                !reversed && fits && !fee_revenue_empty,
                                "an over-refund or a refund after reversal posted"
                            );
                            left_payer -= merchant + back;
                            left_merchant -= merchant;
                            left_fee -= back;
                            refunded = true;
                        }
                        Err(LedgerError::RefundOfReversedEntry(_)) => prop_assert!(reversed),
                        Err(LedgerError::RefundExceedsOriginal { .. }) => {
                            prop_assert!(!reversed && !fits);
                        }
                        Err(LedgerError::Posting(mezgeb_core::PostingError::InsufficientFunds {
                            ..
                        })) => prop_assert!(!reversed && fits && fee_revenue_empty),
                        Err(other) => prop_assert!(false, "unexpected {other}"),
                    }
                }
                Attempt::RefundToSomeoneElse { amount } => {
                    let mut draft = EntryDraft::default()
                        .debit(a.merchant, etb(*amount))
                        .credit(a.other, etb(*amount));
                    draft.refunds = Some(payment);
                    let result = ledger.post(entry, draft);
                    prop_assert!(result.is_err(), "a refund to someone else posted");
                }
                Attempt::Reverse => {
                    let authorization = Authorization {
                        reason: ReasonCode::new("booking_error")
                            .map_err(|e| TestCaseError::fail(e.to_string()))?,
                        requested_by: ActorId::new("ops:1")
                            .map_err(|e| TestCaseError::fail(e.to_string()))?,
                        approved_by: None,
                    };
                    match ledger.reverse(payment, entry, authorization) {
                        Ok(_) => {
                            prop_assert!(!reversed && !refunded);
                            reversed = true;
                        }
                        Err(LedgerError::AlreadyReversed(_)) => prop_assert!(reversed),
                        Err(LedgerError::RefundedEntryCannotBeReversed(_)) => {
                            prop_assert!(refunded && !reversed);
                        }
                        Err(other) => prop_assert!(false, "unexpected {other}"),
                    }
                }
            }
            // Exactly the attempts that succeeded added an entry.
            let posted = ledger.entries().len() - before;
            prop_assert!(posted <= 1);
        }

        // Never more back than was paid, on any account.
        let tally = ledger.refunded(payment);
        let back = |account| tally.get(&account).copied().unwrap_or(0);
        prop_assert!(back(a.payer) <= price);
        prop_assert!(back(a.merchant) <= net);
        prop_assert!(back(a.fees) <= fee);
        prop_assert_eq!(back(a.payer), back(a.merchant) + back(a.fees));
        prop_assert_eq!(back(a.merchant), net - left_merchant);
        prop_assert_eq!(back(a.fees), fee - left_fee);
        // Balances follow: the payer has back exactly what the refunds gave,
        // or everything if the payment was reversed.
        let expected_payer = if reversed { price } else { back(a.payer) };
        prop_assert_eq!(balance(&ledger, a.payer), expected_payer);
        prop_assert_eq!(balance(&ledger, a.other), 0);
        check_invariants(&ledger).map_err(|v| TestCaseError::fail(v.to_string()))?;
    }
}

//! Refunds and disputes (step 2.04).

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{CHART, fixtures, trial_balance};
use mezgeb_core::{
    AccountId, ActorId, Authorization, Chart, Currency, EntryDraft, EntryId, LedgerError,
    LineDraft, MemLedger, Money, PostingError, ReasonCode, check_invariants,
};
use mezgeb_ops::{OperationRequest, TEMPLATE_KEY, TemplateError, TemplateRegistry};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn id(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000_000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn request(
    template: &str,
    roles: &[(&str, AccountId)],
    amounts: &[(&str, Money)],
    metadata: &[(&str, &str)],
    refunds: Option<EntryId>,
) -> OperationRequest {
    OperationRequest {
        template: template.into(),
        accounts: roles.iter().map(|(r, a)| ((*r).to_owned(), *a)).collect(),
        amounts: amounts.iter().map(|(n, m)| ((*n).to_owned(), *m)).collect(),
        metadata: metadata
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        refunds,
        ..OperationRequest::default()
    }
}

/// Each worked entry of this step: its template, the roles of its lines in
/// order, and the entry it refunds or disputes.
const BOOKED: &[(&str, &str, &[&str], Option<&str>)] = &[
    (
        "E11 #1",
        "merchant_refund.v1",
        &["merchant_balance", "fee_revenue", "payer_wallet"],
        Some("E08 #1"),
    ),
    (
        "E12 #1",
        "merchant_refund.v1",
        &["merchant_balance", "fee_revenue", "payer_wallet"],
        Some("E07 #1"),
    ),
    (
        "E21 #1",
        "dispute_open.v1",
        &["merchant_balance", "dispute_item"],
        None,
    ),
    (
        "E22 #1",
        "dispute_resolve_payer.v1",
        &["dispute_item", "payer_wallet"],
        Some("E09 #1"),
    ),
    (
        "E23 #1",
        "dispute_open.v1",
        &["merchant_balance", "dispute_item"],
        None,
    ),
    (
        "E24 #1",
        "dispute_resolve_merchant.v1",
        &["dispute_item", "merchant_balance"],
        None,
    ),
];

/// The whole worked story, with E11, E12 and E21–E24 booked by their
/// templates: each draft equals the entry in the docs, the refunds name
/// their payments, the story ends at the trial balance, and the reference
/// ledger's refund tallies are what the docs say was given back.
#[test]
fn each_template_books_its_worked_entry() -> TestResult {
    let chart = Chart::from_toml(CHART)?;
    let fixtures = fixtures()?;
    let registry = TemplateRegistry::standard();
    let mut ledger = MemLedger::new();
    let mut ids = BTreeMap::new();
    for line in fixtures.iter().flat_map(|f| &f.lines) {
        if !ids.contains_key(&line.reference) {
            let account = id(ids.len() as u64)?;
            ledger.open_account(account, &chart, &line.reference)?;
            ids.insert(line.reference.clone(), account);
        }
    }

    let mut posted: BTreeMap<String, EntryId> = BTreeMap::new();
    let mut booked = 0;
    for (n, fixture) in fixtures.iter().enumerate() {
        let lines: Vec<LineDraft> = fixture
            .lines
            .iter()
            .map(|l| LineDraft {
                account: ids[&l.reference],
                side: l.side,
                amount: l.amount,
            })
            .collect();
        let draft = match BOOKED.iter().find(|(label, ..)| *label == fixture.label) {
            None => EntryDraft {
                lines,
                ..EntryDraft::default()
            },
            Some((label, template, roles, refunds)) => {
                let bound: Vec<(&str, AccountId)> = roles
                    .iter()
                    .zip(&lines)
                    .map(|(role, line)| (*role, line.account))
                    .collect();
                let mut amounts = vec![("amount", fixture.total)];
                if *template == "merchant_refund.v1" {
                    amounts.push(("fee_refund", lines[1].amount));
                }
                let disputed = [("disputed_entry", "payment")];
                let metadata: &[(&str, &str)] = if *template == "dispute_open.v1" {
                    &disputed
                } else {
                    &[]
                };
                let original = refunds.map(|r| posted[r]);
                let draft = registry
                    .draft(
                        &request(template, &bound, &amounts, metadata, original),
                        &ledger,
                    )
                    .map_err(|e| format!("{label}: {e}"))?;
                assert_eq!(draft.lines, lines, "{label}");
                assert_eq!(draft.refunds, original, "{label}");
                assert_eq!(draft.metadata[TEMPLATE_KEY], *template, "{label}");
                booked += 1;
                draft
            }
        };
        let id = entry(n as u64)?;
        ledger
            .post(id, draft)
            .map_err(|e| format!("{}: {e}", fixture.label))?;
        posted.insert(fixture.label.clone(), id);
    }
    assert_eq!(booked, BOOKED.len());

    for (reference, balance) in trial_balance()? {
        let account = ledger.account(ids[&reference]).ok_or("missing account")?;
        assert_eq!(account.balance(), balance, "{reference}");
    }
    check_invariants(&ledger).map_err(|v| v.to_string())?;

    // E09 (Hana paid 50, the merchant got 49): the dispute decided for Hana
    // gave back 49, so 1 ETB is all a refund could still give her.
    let e09 = ledger.refunded(posted["E09 #1"]);
    assert_eq!(e09, BTreeMap::from([(ids["2010:cust-hana"], 4_900)]));
    // E07 (100 paid): E12 gave back 40, of which 39.20 from the merchant.
    let e07 = ledger.refunded(posted["E07 #1"]);
    assert_eq!(e07[&ids["2010:cust-abebe"]], 4_000);
    assert_eq!(e07[&ids["2030:merch-coffee"]], 3_920);
    Ok(())
}

/// A payer, a merchant and fee revenue, after a 100.00 payment with a 2.00
/// fee (entry 2).
struct Shop {
    ledger: MemLedger,
    payer: AccountId,
    other: AccountId,
    merchant: AccountId,
    fees: AccountId,
    payment: EntryId,
    registry: TemplateRegistry,
    chart: Chart,
    next: u64,
}

impl Shop {
    fn new() -> Result<Shop, Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(CHART)?;
        let mut ledger = MemLedger::new();
        let (bank, payer, other, merchant, fees) = (id(1)?, id(2)?, id(3)?, id(4)?, id(5)?);
        ledger.open_account(bank, &chart, "1010")?;
        ledger.open_account(payer, &chart, "2010:cust-abebe")?;
        ledger.open_account(other, &chart, "2010:cust-hana")?;
        ledger.open_account(merchant, &chart, "2030:merch-coffee")?;
        ledger.open_account(fees, &chart, "4010")?;
        ledger.post(
            entry(1)?,
            EntryDraft::default()
                .debit(bank, etb(100_000))
                .credit(payer, etb(100_000)),
        )?;
        ledger.post(
            entry(2)?,
            EntryDraft::default()
                .debit(payer, etb(10_000))
                .credit(merchant, etb(9_800))
                .credit(fees, etb(200)),
        )?;
        Ok(Shop {
            ledger,
            payer,
            other,
            merchant,
            fees,
            payment: entry(2)?,
            registry: TemplateRegistry::standard(),
            chart,
            next: 10,
        })
    }

    fn dispute_item(&mut self) -> Result<AccountId, Box<dyn std::error::Error>> {
        self.next += 1;
        let item = id(100 + self.next)?;
        self.ledger
            .open_account(item, &self.chart, &format!("2060:dsp-{}", self.next))?;
        Ok(item)
    }

    /// Drafts with the template and posts on the reference ledger.
    fn book(&mut self, request: &OperationRequest) -> Result<(), Refused> {
        let draft = self
            .registry
            .draft(request, &self.ledger)
            .map_err(Refused::Template)?;
        self.next += 1;
        let id = entry(self.next).map_err(|_| Refused::Template(TemplateError::AmountOverflow))?;
        self.ledger
            .post(id, draft)
            .map(|_| ())
            .map_err(Refused::Ledger)
    }

    fn refund(&self, amount: i64, fee_refund: i64, to: AccountId) -> OperationRequest {
        request(
            "merchant_refund.v1",
            &[
                ("merchant_balance", self.merchant),
                ("fee_revenue", self.fees),
                ("payer_wallet", to),
            ],
            &[("amount", etb(amount)), ("fee_refund", etb(fee_refund))],
            &[],
            Some(self.payment),
        )
    }

    fn balance(&self, account: AccountId) -> i64 {
        self.ledger
            .account(account)
            .map_or(0, |a| a.balance().amount())
    }
}

#[derive(Debug, PartialEq)]
enum Refused {
    Template(TemplateError),
    Ledger(LedgerError),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Refused {}

#[test]
fn refund_fee_policy_is_the_clients() -> TestResult {
    // The fee returned in full, as in E11.
    let mut shop = Shop::new()?;
    shop.book(&shop.refund(10_000, 200, shop.payer))?;
    assert_eq!(
        (
            shop.balance(shop.payer),
            shop.balance(shop.merchant),
            shop.balance(shop.fees)
        ),
        (100_000, 0, 0)
    );
    // The fee kept: the merchant pays back the whole refund.
    let mut shop = Shop::new()?;
    let keep_fee = request(
        "merchant_refund.v1",
        &[
            ("merchant_balance", shop.merchant),
            ("payer_wallet", shop.payer),
        ],
        &[("amount", etb(9_800))],
        &[],
        Some(shop.payment),
    );
    shop.book(&keep_fee)?;
    assert_eq!(shop.balance(shop.merchant), 0);
    assert_eq!(shop.balance(shop.fees), 200);
    // More fee than the payment earned is refused by the refund rule.
    let mut shop = Shop::new()?;
    assert_eq!(
        shop.book(&shop.refund(1_000, 201, shop.payer)),
        Err(Refused::Ledger(LedgerError::RefundExceedsOriginal {
            line: 1
        }))
    );
    Ok(())
}

#[test]
fn refund_templates_check_their_requests() -> TestResult {
    let mut shop = Shop::new()?;
    let code = |r: Result<(), Refused>| match r {
        Err(Refused::Template(e)) => Some(e.code()),
        _ => None,
    };
    // A refund without the payment it refunds.
    let mut missing = shop.refund(100, 0, shop.payer);
    missing.refunds = None;
    assert_eq!(code(shop.book(&missing)), Some(2021));
    // Any other template naming a payment to refund.
    let mut p2p = request(
        "p2p_transfer.v1",
        &[("payer_wallet", shop.payer), ("payee_wallet", shop.other)],
        &[("amount", etb(100))],
        &[],
        None,
    );
    p2p.refunds = Some(shop.payment);
    assert_eq!(code(shop.book(&p2p)), Some(2022));
    // A fee refund above the refund, or in another currency.
    assert_eq!(
        code(shop.book(&shop.refund(100, 101, shop.payer))),
        Some(2016)
    );
    let mut usd = shop.refund(100, 0, shop.payer);
    usd.amounts
        .insert("fee_refund".into(), Money::from_minor(1, Currency::USD));
    assert_eq!(code(shop.book(&usd)), Some(2017));
    // The payer role takes only a customer wallet.
    assert_eq!(code(shop.book(&shop.refund(100, 0, shop.fees))), Some(2010));
    Ok(())
}

#[test]
fn refunds_never_exceed_the_payment_or_go_to_someone_else() -> TestResult {
    let mut shop = Shop::new()?;
    shop.book(&shop.refund(4_000, 80, shop.payer))?;
    shop.book(&shop.refund(6_000, 120, shop.payer))?;
    // One santim more.
    assert_eq!(
        shop.book(&shop.refund(1, 0, shop.payer)),
        Err(Refused::Ledger(LedgerError::RefundExceedsOriginal {
            line: 0
        }))
    );
    // To someone who never paid.
    let mut shop = Shop::new()?;
    assert_eq!(
        shop.book(&shop.refund(1_000, 0, shop.other)),
        Err(Refused::Ledger(LedgerError::RefundLineNotAllowed {
            line: 1
        }))
    );
    Ok(())
}

fn open(shop: &Shop, item: AccountId, amount: i64) -> OperationRequest {
    request(
        "dispute_open.v1",
        &[("merchant_balance", shop.merchant), ("dispute_item", item)],
        &[("amount", etb(amount))],
        &[("disputed_entry", "payment-2")],
        None,
    )
}

fn for_payer(shop: &Shop, item: AccountId, amount: i64, payer: AccountId) -> OperationRequest {
    request(
        "dispute_resolve_payer.v1",
        &[("dispute_item", item), ("payer_wallet", payer)],
        &[("amount", etb(amount))],
        &[],
        Some(shop.payment),
    )
}

fn for_merchant(shop: &Shop, item: AccountId, amount: i64) -> OperationRequest {
    request(
        "dispute_resolve_merchant.v1",
        &[("dispute_item", item), ("merchant_balance", shop.merchant)],
        &[("amount", etb(amount))],
        &[],
        None,
    )
}

#[test]
fn a_dispute_holds_the_merchants_money_until_one_side_wins() -> TestResult {
    let mut shop = Shop::new()?;
    let item = shop.dispute_item()?;
    shop.book(&open(&shop, item, 9_800))?;
    assert_eq!(
        (shop.balance(shop.merchant), shop.balance(item)),
        (0, 9_800)
    );
    // The merchant cannot be paid out the held money: its balance is zero.
    // Decided for the payer, the whole item goes to the payer.
    assert_eq!(
        shop.book(&for_payer(&shop, item, 9_799, shop.payer))
            .map_err(|e| e.to_string()),
        Err(Refused::Template(TemplateError::AmountNotBalance("dispute_item".into())).to_string())
    );
    shop.book(&for_payer(&shop, item, 9_800, shop.payer))?;
    assert_eq!(shop.balance(item), 0);
    assert_eq!(shop.balance(shop.payer), 90_000 + 9_800);
    assert_eq!(shop.balance(shop.fees), 200, "the platform keeps its fee");
    // Decided twice: refused, the item is empty.
    assert!(shop.book(&for_merchant(&shop, item, 9_800)).is_err());
    // The payer has had 98.00 of the 100.00 back: a refund can give only
    // the 2.00 that is left, from the fee.
    assert_eq!(
        shop.book(&shop.refund(201, 201, shop.payer)),
        Err(Refused::Ledger(LedgerError::RefundExceedsOriginal {
            line: 0
        }))
    );
    shop.book(&shop.refund(200, 200, shop.payer))?;
    assert_eq!(shop.balance(shop.payer), 100_000);
    check_invariants(&shop.ledger).map_err(|v| v.to_string())?;

    // Decided for the merchant: everything goes back, and nothing counts
    // against the payment's refund limit.
    let mut shop = Shop::new()?;
    let item = shop.dispute_item()?;
    shop.book(&open(&shop, item, 5_000))?;
    shop.book(&for_merchant(&shop, item, 5_000))?;
    assert_eq!(shop.balance(shop.merchant), 9_800);
    assert!(shop.ledger.refunded(shop.payment).is_empty());
    // A dispute item is opened once.
    assert_eq!(
        shop.book(&open(&shop, item, 100)),
        Err(Refused::Template(TemplateError::AccountNotNew(
            "dispute_item".into()
        )))
    );
    // And a dispute needs the payment it is about.
    let fresh = shop.dispute_item()?;
    let mut no_entry = open(&shop, fresh, 100);
    no_entry.metadata.clear();
    assert_eq!(
        shop.book(&no_entry),
        Err(Refused::Template(TemplateError::MissingMetadata(
            "disputed_entry".into()
        )))
    );
    Ok(())
}

/// The policy for a dispute the merchant's balance cannot cover: opening it
/// is refused, and nothing changes.
#[test]
fn a_dispute_larger_than_the_merchants_balance_is_refused() -> TestResult {
    let mut shop = Shop::new()?;
    let item = shop.dispute_item()?;
    let before = shop.ledger.entries().len();
    assert_eq!(
        shop.book(&open(&shop, item, 9_801)),
        Err(Refused::Ledger(LedgerError::Posting(
            PostingError::InsufficientFunds { line: 0 }
        )))
    );
    assert_eq!(shop.ledger.entries().len(), before);
    assert_eq!(shop.balance(shop.merchant), 9_800);
    Ok(())
}

#[test]
fn a_dispute_cannot_pay_twice_or_pay_someone_else() -> TestResult {
    // The payer already refunded in full: the dispute cannot pay again.
    let mut shop = Shop::new()?;
    shop.book(&shop.refund(9_000, 0, shop.payer))?;
    let item = shop.dispute_item()?;
    shop.book(&open(&shop, item, 800))?;
    shop.book(&for_payer(&shop, item, 800, shop.payer))?;
    let item = shop.dispute_item()?;
    // Merchant balance is 0 now; give it some money so the dispute can open.
    shop.ledger.post(
        entry(500)?,
        EntryDraft::default()
            .debit(shop.payer, etb(5_000))
            .credit(shop.merchant, etb(5_000)),
    )?;
    shop.book(&open(&shop, item, 1_000))?;
    assert_eq!(
        shop.book(&for_payer(&shop, item, 1_000, shop.payer)),
        Err(Refused::Ledger(LedgerError::RefundExceedsOriginal {
            line: 1
        }))
    );
    // Paid to someone who never paid: refused.
    assert_eq!(
        shop.book(&for_payer(&shop, item, 1_000, shop.other)),
        Err(Refused::Ledger(LedgerError::RefundLineNotAllowed {
            line: 1
        }))
    );
    // Decided for the merchant instead.
    shop.book(&for_merchant(&shop, item, 1_000))?;

    // A reversed payment: the dispute cannot be decided for the payer.
    let mut shop = Shop::new()?;
    let item = shop.dispute_item()?;
    shop.book(&open(&shop, item, 100))?;
    shop.ledger.post(
        entry(600)?,
        EntryDraft::default()
            .debit(shop.payer, etb(100))
            .credit(shop.merchant, etb(100)),
    )?;
    shop.ledger.reverse(
        shop.payment,
        entry(601)?,
        Authorization {
            reason: ReasonCode::new("booking_error")?,
            requested_by: ActorId::new("ops:1")?,
            approved_by: None,
        },
    )?;
    assert_eq!(
        shop.book(&for_payer(&shop, item, 100, shop.payer)),
        Err(Refused::Ledger(LedgerError::RefundOfReversedEntry(
            shop.payment
        )))
    );
    shop.book(&for_merchant(&shop, item, 100))?;
    check_invariants(&shop.ledger).map_err(|v| v.to_string())?;
    Ok(())
}

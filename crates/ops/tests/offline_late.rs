//! The step 2.13 offline templates against their worked entries: returning
//! escrow (E18, E25, E27), paying receipts escrow does not cover (E26, E28,
//! E29, E31) and repaying a shortfall (E30).

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{CHART, Fixture, fixtures, offline_state_fixtures};
use mezgeb_core::{
    AccountId, AccountType, Chart, EntryDraft, EntryId, LineDraft, MemLedger, Money, Side,
};
use mezgeb_ops::{ENGINE_ONLY, OperationRequest, TEMPLATE_KEY, TemplateError, TemplateRegistry};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Ids = BTreeMap<String, AccountId>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn lines(fixture: &Fixture, ids: &Ids) -> Vec<LineDraft> {
    fixture
        .lines
        .iter()
        .map(|l| LineDraft {
            account: ids[&l.reference],
            side: l.side,
            amount: l.amount,
        })
        .collect()
}

/// A ledger holding exactly what one standalone example needs before it
/// posts: each account it debits on its normal credit side, or credits on
/// its normal debit side, is funded by that amount from 3010.
fn ledger_for(fixture: &Fixture) -> Result<(MemLedger, Ids), Box<dyn std::error::Error>> {
    let chart = Chart::from_toml(CHART)?;
    let mut ledger = MemLedger::new();
    let mut ids = Ids::new();
    for reference in fixture
        .lines
        .iter()
        .map(|l| l.reference.as_str())
        .chain(["3010", "1010"])
    {
        if !ids.contains_key(reference) {
            let id: AccountId = uuid7(ids.len() as u64).parse()?;
            ledger.open_account(id, &chart, reference)?;
            ids.insert(reference.to_owned(), id);
        }
    }
    for (n, line) in fixture.lines.iter().enumerate() {
        let account = ids[&line.reference];
        let normal_credit = matches!(
            line.account_type,
            AccountType::Liability | AccountType::Equity | AccountType::Revenue
        );
        let seed = match (normal_credit, line.side) {
            // Give a liability the balance it is debited from.
            (true, Side::Debit) => EntryDraft::default()
                .debit(ids["1010"], line.amount)
                .credit(account, line.amount),
            // Give an expense the balance it is credited from.
            (false, Side::Credit) => EntryDraft::default()
                .debit(account, line.amount)
                .credit(ids["3010"], line.amount),
            _ => continue,
        };
        let id: EntryId = uuid7(1_000 + n as u64).parse()?;
        ledger.post(id, seed)?;
    }
    Ok((ledger, ids))
}

fn example(label: &str) -> Result<Fixture, Box<dyn std::error::Error>> {
    Ok(offline_state_fixtures()?
        .into_iter()
        .find(|f| f.label == label)
        .ok_or(format!("no {label}"))?)
}

fn request(
    template: &str,
    roles: &[(&str, AccountId)],
    amounts: &[(&str, Money)],
    metadata: &[(&str, &str)],
) -> OperationRequest {
    OperationRequest {
        template: template.into(),
        accounts: roles.iter().map(|(r, a)| ((*r).to_owned(), *a)).collect(),
        amounts: amounts.iter().map(|(n, m)| ((*n).to_owned(), *m)).collect(),
        metadata: metadata
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..OperationRequest::default()
    }
}

/// Drafts `request`, checks it equals the example line for line, and posts
/// it.
fn books(label: &str, build: impl Fn(&Ids, &Fixture) -> OperationRequest) -> TestResult {
    let fixture = example(label)?;
    let (mut ledger, ids) = ledger_for(&fixture)?;
    let request = build(&ids, &fixture);
    let draft = TemplateRegistry::standard().draft(&request, &ledger)?;
    assert_eq!(draft.lines, lines(&fixture, &ids), "{label}");
    assert_eq!(draft.metadata[TEMPLATE_KEY], request.template, "{label}");
    ledger.post(uuid7(9_000).parse()?, draft)?;
    Ok(())
}

fn amount(fixture: &Fixture, n: usize) -> Money {
    fixture.lines[n].amount
}

const RECEIPT: &[(&str, &str)] = &[("voucher", "v"), ("device", "d"), ("counter", "7")];

#[test]
fn offline_voucher_return_v1_books_e18_e25_and_e27() -> TestResult {
    // E18, in the running story: the 40 ETB left after E17.
    let chart = Chart::from_toml(CHART)?;
    let story = fixtures()?;
    let mut ledger = MemLedger::new();
    let mut ids = Ids::new();
    for line in story.iter().flat_map(|f| &f.lines) {
        if !ids.contains_key(&line.reference) {
            let id: AccountId = uuid7(ids.len() as u64).parse()?;
            ledger.open_account(id, &chart, &line.reference)?;
            ids.insert(line.reference.clone(), id);
        }
    }
    let mut found = false;
    for (n, fixture) in story.iter().enumerate() {
        if fixture.label == "E18 #1" {
            found = true;
            let r = request(
                "offline_voucher_return.v1",
                &[
                    ("voucher_escrow", ids["2050:vch-0001"]),
                    ("customer_wallet", ids["2010:cust-hana"]),
                ],
                &[("amount", fixture.total)],
                &[("voucher", "vch-0001"), ("reason", "expired")],
            );
            let draft = TemplateRegistry::standard().draft(&r, &ledger)?;
            assert_eq!(draft.lines, lines(fixture, &ids));
            break;
        }
        ledger.post(
            uuid7(2_000 + n as u64).parse()?,
            EntryDraft {
                lines: lines(fixture, &ids),
                ..EntryDraft::default()
            },
        )?;
    }

    assert!(found, "E18 is in the story");

    for (label, reason) in [("E25 #1", "closed"), ("E27 #1", "dispute")] {
        books(label, |ids, f| {
            let escrow = f.lines[0].reference.as_str();
            let wallet = f.lines[1].reference.as_str();
            request(
                "offline_voucher_return.v1",
                &[
                    ("voucher_escrow", ids[escrow]),
                    ("customer_wallet", ids[wallet]),
                ],
                &[("amount", f.total)],
                &[("voucher", "v"), ("reason", reason)],
            )
        })?;
    }
    Ok(())
}

#[test]
fn offline_receipt_pay_v1_books_e26_e28_e29_and_e31() -> TestResult {
    // E26: escrow 30, the payer's wallet 20.
    books("E26 #1", |ids, f| {
        request(
            "offline_receipt_pay.v1",
            &[
                ("voucher_escrow", ids["2050:vch-0003"]),
                ("payer_wallet", ids["2010:cust-hana"]),
                ("payee", ids["2030:merch-coffee"]),
                ("fee_revenue", ids["4010"]),
            ],
            &[
                ("from_escrow", amount(f, 0)),
                ("from_wallet", amount(f, 1)),
                ("merchant_fee", amount(f, 3)),
            ],
            RECEIPT,
        )
    })?;
    // E28 and E31: all from the payer's wallet.
    for (label, wallet) in [("E28 #1", "2010:cust-hana"), ("E31 #1", "2010:cust-sara")] {
        books(label, |ids, f| {
            request(
                "offline_receipt_pay.v1",
                &[
                    ("payer_wallet", ids[wallet]),
                    ("payee", ids["2030:merch-stall"]),
                    ("fee_revenue", ids["4010"]),
                ],
                &[("from_wallet", f.total), ("merchant_fee", amount(f, 2))],
                RECEIPT,
            )
        })?;
    }
    // E29: the wallet's 25, then 65 from the loss account.
    books("E29 #1", |ids, f| {
        request(
            "offline_receipt_pay.v1",
            &[
                ("payer_wallet", ids["2010:cust-hana"]),
                ("offline_loss", ids["5030"]),
                ("payee", ids["2030:merch-stall"]),
                ("fee_revenue", ids["4010"]),
            ],
            &[
                ("from_wallet", amount(f, 0)),
                ("from_loss", amount(f, 1)),
                ("merchant_fee", amount(f, 3)),
            ],
            RECEIPT,
        )
    })?;
    Ok(())
}

#[test]
fn offline_shortfall_repay_v1_books_e30() -> TestResult {
    books("E30 #1", |ids, f| {
        request(
            "offline_shortfall_repay.v1",
            &[
                ("customer_wallet", ids["2010:cust-hana"]),
                ("offline_loss", ids["5030"]),
            ],
            &[("amount", f.total)],
            &[("shortfall", "s-1")],
        )
    })
}

#[test]
fn the_rules_of_the_late_templates() -> TestResult {
    let fixture = example("E29 #1")?;
    let (ledger, ids) = ledger_for(&fixture)?;
    let registry = TemplateRegistry::standard();
    let etb = |minor| Money::from_minor(minor, mezgeb_core::Currency::ETB);
    let payee = ("payee", ids["2030:merch-stall"]);

    // No source at all.
    let none = request("offline_receipt_pay.v1", &[payee], &[], RECEIPT);
    assert_eq!(
        registry.draft(&none, &ledger).err(),
        Some(TemplateError::MissingAmount("from_escrow".into()))
    );
    // A part whose account is not bound.
    let unbound = request(
        "offline_receipt_pay.v1",
        &[payee],
        &[("from_loss", etb(100))],
        RECEIPT,
    );
    assert!(matches!(
        registry.draft(&unbound, &ledger),
        Err(TemplateError::MissingAccount(_))
    ));
    // A fee above the total.
    let fee = request(
        "offline_receipt_pay.v1",
        &[
            payee,
            ("payer_wallet", ids["2010:cust-hana"]),
            ("fee_revenue", ids["4010"]),
        ],
        &[("from_wallet", etb(100)), ("merchant_fee", etb(101))],
        RECEIPT,
    );
    assert_eq!(
        registry.draft(&fee, &ledger).err(),
        Some(TemplateError::FeeExceedsAmount)
    );
    // A return of less than the whole escrow.
    let fixture = example("E25 #1")?;
    let (ledger, ids) = ledger_for(&fixture)?;
    let partial = request(
        "offline_voucher_return.v1",
        &[
            ("voucher_escrow", ids["2050:vch-0002"]),
            ("customer_wallet", ids["2010:cust-abebe"]),
        ],
        &[("amount", etb(100))],
        &[("voucher", "v"), ("reason", "expired")],
    );
    assert!(matches!(
        registry.draft(&partial, &ledger),
        Err(TemplateError::AmountNotBalance(_))
    ));
    // An unknown reason.
    let mut odd = partial.clone();
    odd.amounts.insert("amount".into(), fixture.total);
    odd.metadata.insert("reason".into(), "whim".into());
    assert!(matches!(
        registry.draft(&odd, &ledger),
        Err(TemplateError::InvalidMetadata { .. })
    ));
    for id in [
        "offline_voucher_return.v1",
        "offline_receipt_pay.v1",
        "offline_shortfall_repay.v1",
    ] {
        assert!(ENGINE_ONLY.contains(&id), "{id}");
    }
    Ok(())
}

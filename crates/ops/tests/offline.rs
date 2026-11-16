//! `issue_voucher.v1` against worked entry E16 (step 2.11).

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{CHART, fixtures};
use mezgeb_core::{
    AccountId, Chart, Currency, EntryDraft, EntryId, LineDraft, MemLedger, Money, PostingError,
    validate,
};
use mezgeb_ops::{OperationRequest, TEMPLATE_KEY, TemplateError, TemplateRegistry};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Ids = BTreeMap<String, AccountId>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

/// The story posted up to, not including, E16, with every account it uses
/// and one fresh escrow for a second voucher.
fn before_e16() -> Result<(MemLedger, Ids, common::Fixture), Box<dyn std::error::Error>> {
    let chart = Chart::from_toml(CHART)?;
    let fixtures = fixtures()?;
    let mut ledger = MemLedger::new();
    let mut ids = Ids::new();
    let references = fixtures
        .iter()
        .flat_map(|f| f.lines.iter().map(|l| l.reference.clone()))
        .chain(["2050:vch-0002".to_owned()]);
    for reference in references {
        if !ids.contains_key(&reference) {
            let id: AccountId = uuid7(ids.len() as u64).parse()?;
            ledger.open_account(id, &chart, &reference)?;
            ids.insert(reference, id);
        }
    }
    for (n, fixture) in fixtures.iter().enumerate() {
        if fixture.label == "E16 #1" {
            return Ok((ledger, ids, fixture.clone()));
        }
        let lines = fixture
            .lines
            .iter()
            .map(|l| LineDraft {
                account: ids[&l.reference],
                side: l.side,
                amount: l.amount,
            })
            .collect();
        let id: EntryId = uuid7(1_000 + n as u64).parse()?;
        ledger.post(
            id,
            EntryDraft {
                lines,
                ..EntryDraft::default()
            },
        )?;
    }
    Err("no E16".into())
}

fn issue(wallet: AccountId, escrow: AccountId, amount: Money, voucher: &str) -> OperationRequest {
    OperationRequest {
        template: "issue_voucher.v1".into(),
        accounts: BTreeMap::from([
            ("customer_wallet".into(), wallet),
            ("voucher_escrow".into(), escrow),
        ]),
        amounts: BTreeMap::from([("amount".into(), amount)]),
        metadata: BTreeMap::from([("voucher".into(), voucher.into())]),
        ..OperationRequest::default()
    }
}

#[test]
fn issue_voucher_v1_books_worked_entry_e16() -> TestResult {
    let (mut ledger, ids, e16) = before_e16()?;
    let wallet = ids["2010:cust-hana"];
    let escrow = ids["2050:vch-0001"];
    let request = issue(wallet, escrow, e16.total, "vch-0001");

    let draft = TemplateRegistry::standard().draft(&request, &ledger)?;
    let expected: Vec<LineDraft> = e16
        .lines
        .iter()
        .map(|l| LineDraft {
            account: ids[&l.reference],
            side: l.side,
            amount: l.amount,
        })
        .collect();
    assert_eq!(draft.lines, expected);
    assert_eq!(draft.metadata[TEMPLATE_KEY], "issue_voucher.v1");
    assert_eq!(draft.metadata["voucher"], "vch-0001");

    ledger.post(uuid7(9_000).parse()?, draft)?;
    assert_eq!(ledger.account(escrow).map(|a| a.balance()), Some(e16.total));

    // The escrow is used: a second voucher cannot be funded into it.
    assert!(matches!(
        TemplateRegistry::standard().draft(&request, &ledger),
        Err(TemplateError::AccountNotNew(_))
    ));
    Ok(())
}

#[test]
fn a_voucher_needs_its_voucher_and_wallet_funds() -> TestResult {
    let (ledger, ids, _) = before_e16()?;
    let wallet = ids["2010:cust-hana"];
    let escrow = ids["2050:vch-0002"];
    let registry = TemplateRegistry::standard();

    let mut no_voucher = issue(wallet, escrow, Money::from_minor(100, Currency::ETB), "v");
    no_voucher.metadata.clear();
    assert!(registry.draft(&no_voucher, &ledger).is_err());

    // More than Hana's 680 ETB: the posting rules refuse it (INV-8).
    let too_much = issue(
        wallet,
        escrow,
        Money::from_minor(68_001, Currency::ETB),
        "v",
    );
    let draft = registry.draft(&too_much, &ledger)?;
    assert!(matches!(
        validate(draft, &ledger),
        Err(PostingError::InsufficientFunds { .. })
    ));

    // Only a customer wallet funds a voucher, only into offline escrow.
    let swapped = issue(escrow, wallet, Money::from_minor(100, Currency::ETB), "v");
    assert!(registry.draft(&swapped, &ledger).is_err());
    Ok(())
}

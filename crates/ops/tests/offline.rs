//! `issue_voucher.v1` against worked entry E16 (step 2.11).

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{CHART, fixtures};
use mezgeb_core::{
    AccountId, Chart, Currency, EntryDraft, EntryId, LineDraft, MemLedger, Money, PostingError,
    validate,
};
use mezgeb_ops::{ENGINE_ONLY, OperationRequest, TEMPLATE_KEY, TemplateError, TemplateRegistry};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Ids = BTreeMap<String, AccountId>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

/// The story posted up to, not including, `label`, with every account it
/// uses and one fresh escrow for a second voucher.
fn before(label: &str) -> Result<(MemLedger, Ids, common::Fixture), Box<dyn std::error::Error>> {
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
        if fixture.label == label {
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
    Err(format!("no {label}").into())
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
    let (mut ledger, ids, e16) = before("E16 #1")?;
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
    let (ledger, ids, _) = before("E16 #1")?;
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

fn settle(
    escrow: AccountId,
    payee: AccountId,
    fee: Option<(AccountId, Money)>,
    amount: Money,
) -> OperationRequest {
    let mut accounts = BTreeMap::from([
        ("voucher_escrow".to_owned(), escrow),
        ("payee".to_owned(), payee),
    ]);
    let mut amounts = BTreeMap::from([("amount".to_owned(), amount)]);
    if let Some((revenue, fee)) = fee {
        accounts.insert("fee_revenue".into(), revenue);
        amounts.insert("merchant_fee".into(), fee);
    }
    OperationRequest {
        template: "offline_receipt_settle.v1".into(),
        accounts,
        amounts,
        metadata: BTreeMap::from([
            ("voucher".into(), "vch-0001".into()),
            ("device".into(), "dev-hana-01".into()),
            ("counter".into(), "1".into()),
        ]),
        ..OperationRequest::default()
    }
}

#[test]
fn offline_receipt_settle_v1_books_worked_entry_e17() -> TestResult {
    let (mut ledger, ids, e17) = before("E17 #1")?;
    let escrow = ids["2050:vch-0001"];
    let payee = ids["2030:merch-coffee"];
    let fee = e17.lines[2].amount;
    let request = settle(escrow, payee, Some((ids["4010"], fee)), e17.total);

    let draft = TemplateRegistry::standard().draft(&request, &ledger)?;
    let expected: Vec<LineDraft> = e17
        .lines
        .iter()
        .map(|l| LineDraft {
            account: ids[&l.reference],
            side: l.side,
            amount: l.amount,
        })
        .collect();
    assert_eq!(draft.lines, expected);
    assert_eq!(draft.metadata[TEMPLATE_KEY], "offline_receipt_settle.v1");
    assert_eq!(draft.metadata["counter"], "1");
    ledger.post(uuid7(9_001).parse()?, draft)?;
    assert_eq!(
        ledger.account(escrow).map(|a| a.balance()),
        Some(Money::from_minor(4_000, Currency::ETB))
    );
    Ok(())
}

#[test]
fn a_receipt_settles_only_what_the_escrow_holds() -> TestResult {
    let (ledger, ids, _) = before("E17 #1")?;
    let registry = TemplateRegistry::standard();
    let escrow = ids["2050:vch-0001"];
    let wallet = ids["2010:cust-abebe"];

    // Without a fee, the payee gets it all; a customer can be paid too.
    let plain = registry.draft(
        &settle(
            escrow,
            wallet,
            None,
            Money::from_minor(1_000, Currency::ETB),
        ),
        &ledger,
    )?;
    assert_eq!(plain.lines.len(), 2);

    // More than the 100 ETB in escrow: the posting rules refuse it.
    let over = registry.draft(
        &settle(
            escrow,
            wallet,
            None,
            Money::from_minor(10_001, Currency::ETB),
        ),
        &ledger,
    )?;
    assert!(matches!(
        validate(over, &ledger),
        Err(PostingError::InsufficientFunds { .. })
    ));

    // Only from offline escrow, only to a wallet or a merchant.
    assert!(
        registry
            .draft(
                &settle(wallet, escrow, None, Money::from_minor(100, Currency::ETB)),
                &ledger
            )
            .is_err()
    );
    // A fee larger than the amount is refused.
    let too_big = settle(
        escrow,
        wallet,
        Some((ids["4010"], Money::from_minor(200, Currency::ETB))),
        Money::from_minor(100, Currency::ETB),
    );
    assert_eq!(
        registry.draft(&too_big, &ledger).err(),
        Some(TemplateError::FeeExceedsAmount)
    );
    // It names what it settles.
    let mut unnamed = settle(escrow, wallet, None, Money::from_minor(100, Currency::ETB));
    unnamed.metadata.remove("counter");
    assert!(registry.draft(&unnamed, &ledger).is_err());
    assert!(ENGINE_ONLY.contains(&"offline_receipt_settle.v1"));
    Ok(())
}

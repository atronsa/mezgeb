//! Posting templates against the worked entries and the posting rules
//! (step 2.01).

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{CHART, Fixture, fixtures};
use mezgeb_core::{
    AccountId, Chart, Currency, EntryDraft, EntryId, LineDraft, MemLedger, Money, PositiveAmount,
    Side, Timestamp, validate,
};
use mezgeb_ops::{
    Input, Leg, OperationRequest, P2pTransferRequest, P2pTransferV1, RELEASED, RegisterError, Role,
    TEMPLATE_KEY, Template, TemplateError, TemplateRegistry, draft,
};
use proptest::prelude::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Ids = BTreeMap<String, AccountId>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// A ledger with every account the worked entries use, and every entry
/// before `label` posted, as the story stood just before it.
fn ledger_before(label: &str) -> Result<(MemLedger, Ids, Fixture), Box<dyn std::error::Error>> {
    let chart = Chart::from_toml(CHART)?;
    let fixtures = fixtures()?;
    let mut ledger = MemLedger::new();
    let mut ids = Ids::new();
    for line in fixtures.iter().flat_map(|f| &f.lines) {
        if !ids.contains_key(&line.reference) {
            let id: AccountId = uuid7(ids.len() as u64).parse()?;
            ledger.open_account(id, &chart, &line.reference)?;
            ids.insert(line.reference.clone(), id);
        }
    }
    let mut entry = 0;
    for fixture in &fixtures {
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
        entry += 1;
        let id: EntryId = uuid7(1_000 + entry).parse()?;
        ledger.post(
            id,
            EntryDraft {
                lines,
                ..EntryDraft::default()
            },
        )?;
    }
    Err(format!("no worked entry {label}").into())
}

fn p2p(payer: AccountId, payee: AccountId, amount: Money) -> OperationRequest {
    OperationRequest {
        template: "p2p_transfer.v1".into(),
        accounts: BTreeMap::from([
            ("payer_wallet".into(), payer),
            ("payee_wallet".into(), payee),
        ]),
        amounts: BTreeMap::from([("amount".into(), amount)]),
        ..OperationRequest::default()
    }
}

#[test]
fn p2p_transfer_v1_books_worked_entry_e06() -> TestResult {
    let (mut ledger, ids, e06) = ledger_before("E06 #1")?;
    let abebe = ids["2010:cust-abebe"];
    let hana = ids["2010:cust-hana"];
    let mut request = p2p(abebe, hana, e06.total);
    request.metadata.insert("payment".into(), "p2p-1".into());

    let draft = TemplateRegistry::standard().draft(&request, &ledger)?;

    let expected: Vec<LineDraft> = e06
        .lines
        .iter()
        .map(|l| LineDraft {
            account: ids[&l.reference],
            side: l.side,
            amount: l.amount,
        })
        .collect();
    assert_eq!(draft.lines, expected);
    assert_eq!(
        draft.metadata,
        BTreeMap::from([
            ("payment".into(), "p2p-1".into()),
            (TEMPLATE_KEY.into(), "p2p_transfer.v1".into()),
        ])
    );
    assert!(!draft.allow_duplicate_lines);

    // It posts, and the entry keeps the template that booked it.
    let posted = ledger.post(uuid7(9_000).parse()?, draft)?;
    assert_eq!(
        posted.metadata().get(TEMPLATE_KEY).map(String::as_str),
        Some("p2p_transfer.v1")
    );
    assert_eq!(
        ledger.account(abebe).map(|a| a.balance()),
        Some(etb(85_000))
    );
    assert_eq!(ledger.account(hana).map(|a| a.balance()), Some(etb(15_000)));
    Ok(())
}

#[test]
fn the_typed_request_books_the_same_draft() -> TestResult {
    let (ledger, ids, e06) = ledger_before("E06 #1")?;
    let request = p2p(ids["2010:cust-abebe"], ids["2010:cust-hana"], e06.total);
    let typed = P2pTransferRequest {
        amount: PositiveAmount::new(e06.total)?,
    };
    let direct =
        draft::<P2pTransferV1, _>(&typed, &request.accounts, BTreeMap::new(), None, &ledger)?;
    assert_eq!(
        direct,
        TemplateRegistry::standard().draft(&request, &ledger)?
    );
    Ok(())
}

/// Each rule's error, in the order of the table in `TemplateError`.
#[test]
fn every_refusal_has_its_stable_code() -> TestResult {
    let (ledger, ids, _) = ledger_before("E06 #1")?;
    let registry = TemplateRegistry::standard();
    let abebe = ids["2010:cust-abebe"];
    let hana = ids["2010:cust-hana"];
    let bank = ids["1010"];
    let nobody: AccountId = uuid7(999_999).parse()?;
    let ok = || p2p(abebe, hana, etb(100));

    type Change = Box<dyn Fn(&mut OperationRequest)>;
    let cases: Vec<(u16, &str, Change)> = vec![
        (
            2001,
            "malformed id",
            Box::new(|r| r.template = "p2p_transfer".into()),
        ),
        (2001, "empty id", Box::new(|r| r.template.clear())),
        (
            2002,
            "unknown template",
            Box::new(|r| r.template = "teleport.v1".into()),
        ),
        (
            2003,
            "unknown version",
            Box::new(|r| r.template = "p2p_transfer.v2".into()),
        ),
        (2004, "missing amount", Box::new(|r| r.amounts.clear())),
        (
            2005,
            "extra amount",
            Box::new(|r| {
                r.amounts.insert("fee".into(), etb(1));
            }),
        ),
        (
            2006,
            "zero amount",
            Box::new(|r| {
                r.amounts.insert("amount".into(), etb(0));
            }),
        ),
        (
            2006,
            "negative amount",
            Box::new(|r| {
                r.amounts.insert("amount".into(), etb(-5));
            }),
        ),
        (
            2007,
            "missing role",
            Box::new(|r| {
                r.accounts.remove("payee_wallet");
            }),
        ),
        (
            2008,
            "extra role",
            Box::new(move |r| {
                r.accounts.insert("fee_revenue".into(), hana);
            }),
        ),
        (
            2009,
            "unknown account",
            Box::new(move |r| {
                r.accounts.insert("payee_wallet".into(), nobody);
            }),
        ),
        (
            2010,
            "wrong kind of account",
            Box::new(move |r| {
                r.accounts.insert("payer_wallet".into(), bank);
            }),
        ),
        (
            2011,
            "same account twice",
            Box::new(move |r| {
                r.accounts.insert("payee_wallet".into(), abebe);
            }),
        ),
        (
            2012,
            "reserved metadata",
            Box::new(|r| {
                r.metadata
                    .insert(TEMPLATE_KEY.into(), "p2p_transfer.v9".into());
            }),
        ),
    ];

    assert!(registry.draft(&ok(), &ledger).is_ok());
    for (code, name, change) in cases {
        let mut request = ok();
        change(&mut request);
        let result = registry.draft(&request, &ledger);
        assert_eq!(result.map_err(|e| e.code()), Err(code), "{name}");
    }
    Ok(())
}

#[test]
fn refusals_name_what_is_wrong() -> TestResult {
    let (ledger, ids, _) = ledger_before("E06 #1")?;
    let registry = TemplateRegistry::standard();
    let abebe = ids["2010:cust-abebe"];
    let mut request = p2p(abebe, abebe, etb(100));
    assert_eq!(
        registry.draft(&request, &ledger),
        Err(TemplateError::SameAccount {
            role: "payee_wallet".into(),
            other: "payer_wallet".into(),
        })
    );
    request.accounts.clear();
    assert_eq!(
        registry.draft(&request, &ledger),
        Err(TemplateError::MissingAccount("payer_wallet".into()))
    );
    Ok(())
}

#[test]
fn the_template_does_not_check_funds_the_posting_rules_do() -> TestResult {
    let (ledger, ids, _) = ledger_before("E06 #1")?;
    // Hana's wallet is empty before E06.
    let request = p2p(ids["2010:cust-hana"], ids["2010:cust-abebe"], etb(100));
    let draft = TemplateRegistry::standard().draft(&request, &ledger)?;
    assert_eq!(validate(draft, &ledger).map_err(|e| e.code()), Err(1012));
    Ok(())
}

#[test]
fn the_standard_registry_has_every_released_version() {
    let registry = TemplateRegistry::standard();
    let ids: Vec<String> = registry.ids().map(ToString::to_string).collect();
    let mut released: Vec<String> = RELEASED.iter().map(|s| (*s).to_owned()).collect();
    released.sort();
    assert_eq!(ids, released);
}

#[test]
fn the_registry_describes_each_template() -> Result<(), TemplateError> {
    let registry = TemplateRegistry::standard();
    let roles: Vec<&str> = registry
        .roles("p2p_transfer.v1")?
        .iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(roles, ["payer_wallet", "payee_wallet"]);
    assert_eq!(registry.amounts("p2p_transfer.v1")?, ["amount"]);
    assert_eq!(
        registry.roles("p2p_transfer.v2").map_err(|e| e.code()),
        Err(2003)
    );
    Ok(())
}

// A template written outside this crate, against the public API only: the
// exit gate's "a new template can be added without touching the engine".

/// E17-like: sweep collected fee revenue to the operating bank account.
struct FeeSweepV1;

impl Template for FeeSweepV1 {
    const NAME: &'static str = "test_fee_sweep";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("customer_funds", &[1010]),
        Role::new("operating_bank", &[1020]),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = PositiveAmount;

    fn request(input: &Input<'_>) -> Result<PositiveAmount, TemplateError> {
        input.amount("amount")
    }

    fn book(amount: &PositiveAmount) -> Result<Vec<Leg>, TemplateError> {
        Ok(vec![
            Leg::debit("operating_bank", *amount),
            Leg::credit("customer_funds", *amount),
        ])
    }
}

/// Version 2 books the same event differently; v1 stays.
struct FeeSweepV2;

impl Template for FeeSweepV2 {
    const NAME: &'static str = "test_fee_sweep";
    const VERSION: u16 = 2;
    const ROLES: &'static [Role] = FeeSweepV1::ROLES;
    const AMOUNTS: &'static [&'static str] = &["amount", "bank_charge"];
    type Request = (PositiveAmount, PositiveAmount);

    fn request(input: &Input<'_>) -> Result<Self::Request, TemplateError> {
        Ok((input.amount("amount")?, input.amount("bank_charge")?))
    }

    fn book(request: &Self::Request) -> Result<Vec<Leg>, TemplateError> {
        let (amount, _charge) = *request;
        Ok(vec![
            Leg::debit("operating_bank", amount),
            Leg::credit("customer_funds", amount),
        ])
    }
}

/// A defective template: it books a role it never declared.
struct Defective;

impl Template for Defective {
    const NAME: &'static str = "test_defective";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[Role::new("payer_wallet", &[2010])];
    const AMOUNTS: &'static [&'static str] = &["amount"];
    type Request = PositiveAmount;

    fn request(input: &Input<'_>) -> Result<PositiveAmount, TemplateError> {
        input.amount("amount")
    }

    fn book(amount: &PositiveAmount) -> Result<Vec<Leg>, TemplateError> {
        Ok(vec![
            Leg::debit("payer_wallet", *amount),
            Leg::credit("somewhere_else", *amount),
        ])
    }
}

/// A template whose name is not a valid template ID.
struct BadName;

impl Template for BadName {
    const NAME: &'static str = "Bad-Name";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[];
    const AMOUNTS: &'static [&'static str] = &[];
    type Request = ();

    fn request(_: &Input<'_>) -> Result<(), TemplateError> {
        Ok(())
    }

    fn book(_: &()) -> Result<Vec<Leg>, TemplateError> {
        Ok(Vec::new())
    }
}

#[test]
fn a_new_template_and_version_can_be_added_from_outside() -> TestResult {
    let (mut ledger, ids, _) = ledger_before("E06 #1")?;
    let mut registry = TemplateRegistry::standard();
    registry.register::<FeeSweepV1>()?;
    registry.register::<FeeSweepV2>()?;
    assert_eq!(
        registry.register::<FeeSweepV1>(),
        Err(RegisterError::Duplicate("test_fee_sweep.v1".parse()?))
    );
    assert!(matches!(
        registry.register::<BadName>(),
        Err(RegisterError::InvalidId(TemplateError::InvalidTemplateId(
            _
        )))
    ));

    let accounts = BTreeMap::from([
        ("customer_funds".to_owned(), ids["1010"]),
        ("operating_bank".to_owned(), ids["1020"]),
    ]);
    let v1 = OperationRequest {
        template: "test_fee_sweep.v1".into(),
        accounts: accounts.clone(),
        amounts: BTreeMap::from([("amount".into(), etb(500))]),
        event_time: Some(Timestamp::from_micros(7)),
        ..OperationRequest::default()
    };
    let draft = registry.draft(&v1, &ledger)?;
    assert_eq!(draft.event_time, Some(Timestamp::from_micros(7)));
    let posted = ledger.post(uuid7(9_001).parse()?, draft)?;
    assert_eq!(
        posted.metadata().get(TEMPLATE_KEY).map(String::as_str),
        Some("test_fee_sweep.v1")
    );

    // v2 reads an extra amount; each version keeps its own rules.
    let mut v2 = v1.clone();
    v2.template = "test_fee_sweep.v2".into();
    assert_eq!(
        registry.draft(&v2, &ledger).map_err(|e| e.code()),
        Err(2004)
    );
    v2.amounts.insert("bank_charge".into(), etb(3));
    let draft = registry.draft(&v2, &ledger)?;
    assert_eq!(draft.metadata[TEMPLATE_KEY], "test_fee_sweep.v2");
    let mut v1_extra = v1;
    v1_extra.amounts.insert("bank_charge".into(), etb(3));
    assert_eq!(
        registry.draft(&v1_extra, &ledger).map_err(|e| e.code()),
        Err(2005)
    );
    Ok(())
}

#[test]
fn a_template_that_books_an_undeclared_role_is_refused() -> TestResult {
    let (ledger, ids, _) = ledger_before("E06 #1")?;
    let mut registry = TemplateRegistry::new();
    registry.register::<Defective>()?;
    let request = OperationRequest {
        template: "test_defective.v1".into(),
        accounts: BTreeMap::from([("payer_wallet".into(), ids["2010:cust-abebe"])]),
        amounts: BTreeMap::from([("amount".into(), etb(1))]),
        ..OperationRequest::default()
    };
    assert_eq!(
        registry.draft(&request, &ledger),
        Err(TemplateError::UndeclaredRole("somewhere_else".into()))
    );
    Ok(())
}

/// The acceptance criterion: the engine has no knowledge of any template
/// name. No source file of the engine, the core or the schema mentions a
/// released template, and the engine does not depend on this crate.
#[test]
fn the_engine_knows_no_template_name() -> TestResult {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["crates/engine/src", "crates/core/src", "migrations"] {
        let mut stack = vec![root.join(dir)];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path)? {
                    stack.push(entry?.path());
                }
            } else {
                files.push(path);
            }
        }
    }
    assert!(files.len() > 10, "found the sources");
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        for id in RELEASED {
            let id: mezgeb_ops::TemplateId = id.parse()?;
            assert!(
                !text.contains(id.name()),
                "{} mentions template {}",
                file.display(),
                id.name()
            );
        }
    }
    let manifest = std::fs::read_to_string(root.join("crates/engine/Cargo.toml"))?;
    assert!(!manifest.contains("mezgeb-ops"));
    Ok(())
}

// Property: any valid P2P request yields an entry that `validate` accepts.

fn ledger_with_wallets(
    balances: &[i64],
) -> Result<(MemLedger, Vec<AccountId>), Box<dyn std::error::Error>> {
    let chart = Chart::from_toml(CHART)?;
    let mut ledger = MemLedger::new();
    let bank: AccountId = uuid7(0).parse()?;
    ledger.open_account(bank, &chart, "1010")?;
    let mut wallets = Vec::new();
    for (i, balance) in balances.iter().enumerate() {
        let n = i as u64 + 1;
        let id: AccountId = uuid7(n).parse()?;
        ledger.open_account(id, &chart, &format!("2010:cust-{n}"))?;
        if *balance > 0 {
            let deposit = EntryDraft::default()
                .debit(bank, etb(*balance))
                .credit(id, etb(*balance));
            ledger.post(uuid7(10_000 + n).parse()?, deposit)?;
        }
        wallets.push(id);
    }
    Ok((ledger, wallets))
}

proptest! {
    #[test]
    fn any_valid_p2p_request_validates(
        balances in prop::collection::vec(1i64..=1_000_000_000_000, 2..6),
        payer in any::<prop::sample::Index>(),
        payee_offset in any::<prop::sample::Index>(),
        fraction in 1u64..=1_000,
        metadata in prop::collection::btree_map("[a-z_]{1,16}", "[ -~]{0,32}", 0..8),
        event_time in prop::option::of(0i64..=4_102_444_800_000_000),
    ) {
        let (ledger, wallets) = ledger_with_wallets(&balances)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let payer = payer.index(wallets.len());
        // Any wallet but the payer's.
        let payee = (payer + 1 + payee_offset.index(wallets.len() - 1)) % wallets.len();
        let balance = balances[payer];
        let amount = (balance as u128 * u128::from(fraction) / 1_000).max(1) as i64;
        let mut metadata = metadata;
        metadata.remove(TEMPLATE_KEY);

        let mut request = p2p(wallets[payer], wallets[payee], etb(amount));
        request.metadata = metadata.clone();
        request.event_time = event_time.map(Timestamp::from_micros);
        let draft = TemplateRegistry::standard()
            .draft(&request, &ledger)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let entry = validate(draft, &ledger)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;

        prop_assert_eq!(entry.lines().len(), 2);
        prop_assert_eq!(entry.lines()[0].account(), wallets[payer]);
        prop_assert_eq!(entry.lines()[0].side(), Side::Debit);
        prop_assert_eq!(entry.lines()[1].account(), wallets[payee]);
        prop_assert_eq!(entry.lines()[1].side(), Side::Credit);
        prop_assert_eq!(entry.totals(), BTreeMap::from([(Currency::ETB, amount)]));
        metadata.insert(TEMPLATE_KEY.into(), "p2p_transfer.v1".into());
        prop_assert_eq!(entry.metadata(), &metadata);
        prop_assert_eq!(entry.event_time(), request.event_time);
    }

    /// Any request at all, valid or not, is either refused with a template
    /// error or drafted into an entry that names its template.
    #[test]
    fn any_request_is_drafted_or_refused_cleanly(
        template in prop_oneof![
            Just("p2p_transfer.v1".to_owned()),
            Just("p2p_transfer.v2".to_owned()),
            "[a-z0-9_.v]{0,20}",
        ],
        roles in prop::collection::btree_map(
            prop_oneof![Just("payer_wallet".to_owned()), Just("payee_wallet".to_owned()), "[a-z_]{1,8}"],
            0u64..4,
            0..4,
        ),
        amounts in prop::collection::btree_map(
            prop_oneof![Just("amount".to_owned()), "[a-z]{1,6}"],
            -10i64..10,
            0..3,
        ),
    ) {
        let (ledger, _) = ledger_with_wallets(&[100, 100, 100])
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let mut accounts = BTreeMap::new();
        for (role, n) in roles {
            let id: AccountId = uuid7(n).parse().map_err(|e: mezgeb_core::IdError| TestCaseError::fail(e.to_string()))?;
            accounts.insert(role, id);
        }
        let request = OperationRequest {
            template: template.clone(),
            accounts,
            amounts: amounts.into_iter().map(|(k, v)| (k, etb(v))).collect(),
            ..OperationRequest::default()
        };
        match TemplateRegistry::standard().draft(&request, &ledger) {
            Ok(draft) => {
                prop_assert_eq!(template.as_str(), "p2p_transfer.v1");
                prop_assert_eq!(draft.metadata.get(TEMPLATE_KEY), Some(&template));
                prop_assert_eq!(draft.lines.len(), 2);
                prop_assert!(draft.lines.iter().all(|l| l.amount.is_positive()));
            }
            Err(error) => prop_assert!((2001..=2013).contains(&error.code())),
        }
    }
}

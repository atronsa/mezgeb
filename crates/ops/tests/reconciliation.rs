//! Reconciliation suspense templates (step 2.15, ADR-0020): each books its
//! worked entry (E32–E35) and refuses what breaks the policy.

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{CHART, reconciliation_fixtures};
use mezgeb_core::{
    AccountId, Chart, Currency, EntryDraft, EntryId, LineDraft, MemLedger, Money, PostingError,
    Side, validate,
};
use mezgeb_ops::{OperationRequest, TEMPLATE_KEY, TemplateRegistry};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn id(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    format!("00000000-0000-7000-8000-{n:012x}").parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn request(template: &str, roles: &[(&str, AccountId)], amount: Money) -> OperationRequest {
    OperationRequest {
        template: template.into(),
        accounts: roles.iter().map(|(r, a)| ((*r).to_owned(), *a)).collect(),
        amounts: BTreeMap::from([("amount".into(), amount)]),
        ..OperationRequest::default()
    }
}

/// A ledger with every account the examples use, plus 1010 and Abebe's
/// wallet funded by a 1,000 ETB deposit (E01 by hand).
struct Books {
    ledger: MemLedger,
    ids: BTreeMap<String, AccountId>,
    registry: TemplateRegistry,
    posted: u64,
}

impl Books {
    fn new() -> Result<Books, Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(CHART)?;
        let mut books = Books {
            ledger: MemLedger::new(),
            ids: BTreeMap::new(),
            registry: TemplateRegistry::standard(),
            posted: 0,
        };
        let references = reconciliation_fixtures()?
            .iter()
            .flat_map(|f| f.lines.iter().map(|l| l.reference.clone()))
            .chain(["4010".to_owned(), "9040:sbd-0003".to_owned()])
            .collect::<Vec<_>>();
        for reference in references {
            if !books.ids.contains_key(&reference) {
                let account = id(books.ids.len() as u64)?;
                books.ledger.open_account(account, &chart, &reference)?;
                books.ids.insert(reference, account);
            }
        }
        let deposit = EntryDraft {
            lines: vec![
                LineDraft {
                    account: books.id("1010"),
                    side: Side::Debit,
                    amount: etb(100_000),
                },
                LineDraft {
                    account: books.id("2010:cust-abebe"),
                    side: Side::Credit,
                    amount: etb(100_000),
                },
            ],
            ..EntryDraft::default()
        };
        books.post(deposit)?;
        Ok(books)
    }

    fn id(&self, reference: &str) -> AccountId {
        self.ids[reference]
    }

    fn post(&mut self, draft: EntryDraft) -> Result<(), Box<dyn std::error::Error>> {
        self.posted += 1;
        let entry = EntryId::from_uuid(id(10_000 + self.posted)?.as_uuid())?;
        self.ledger.post(entry, draft)?;
        Ok(())
    }

    fn balance(&self, reference: &str) -> i64 {
        self.ledger
            .account(self.id(reference))
            .map_or(0, |a| a.balance().amount())
    }
}

/// The template and roles, in line order, that book each worked entry.
const BOOKED: &[(&str, &str, [&str; 2])] = &[
    (
        "E32 #1",
        "bank_debit_unmatched.v1",
        ["bank_debit_item", "customer_funds"],
    ),
    (
        "E33 #1",
        "bank_debit_resolve.v1",
        ["cause", "bank_debit_item"],
    ),
    (
        "E34 #1",
        "bank_debit_unmatched.v1",
        ["bank_debit_item", "customer_funds"],
    ),
    (
        "E35 #1",
        "bank_debit_resolve.v1",
        ["cause", "bank_debit_item"],
    ),
];

#[test]
fn each_template_books_its_worked_entry() -> TestResult {
    let mut books = Books::new()?;
    let fixtures = reconciliation_fixtures()?;
    assert_eq!(fixtures.len(), BOOKED.len());
    for (fixture, (label, template, roles)) in fixtures.iter().zip(BOOKED) {
        assert_eq!(fixture.label, *label);
        let bound: Vec<(&str, AccountId)> = roles
            .iter()
            .zip(&fixture.lines)
            .map(|(role, line)| (*role, books.id(&line.reference)))
            .collect();
        let draft = books
            .registry
            .draft(&request(template, &bound, fixture.total), &books.ledger)
            .map_err(|e| format!("{label}: {e}"))?;
        let by_hand: Vec<LineDraft> = fixture
            .lines
            .iter()
            .map(|l| LineDraft {
                account: books.id(&l.reference),
                side: l.side,
                amount: l.amount,
            })
            .collect();
        assert_eq!(draft.lines, by_hand, "{label}");
        assert_eq!(draft.metadata[TEMPLATE_KEY], *template, "{label}");
        books.post(draft).map_err(|e| format!("{label}: {e}"))?;
    }

    // Both items are closed; 1010 lost the charge and the duplicate.
    assert_eq!(books.balance("9040:sbd-0001"), 0);
    assert_eq!(books.balance("9040:sbd-0002"), 0);
    assert_eq!(books.balance("5020"), 1_500);
    assert_eq!(books.balance("1010"), 100_000 - 1_500 - 20_000);
    assert_eq!(books.balance("2010:cust-abebe"), 100_000 - 20_000);
    Ok(())
}

#[test]
fn each_step_refuses_what_breaks_the_policy() -> TestResult {
    let mut books = Books::new()?;
    let bank = books.id("1010");
    let item = books.id("9040:sbd-0003");
    let open = |amount| {
        request(
            "bank_debit_unmatched.v1",
            &[("bank_debit_item", item), ("customer_funds", bank)],
            etb(amount),
        )
    };
    let resolve = |cause, amount| {
        request(
            "bank_debit_resolve.v1",
            &[("cause", cause), ("bank_debit_item", item)],
            etb(amount),
        )
    };
    let code = |books: &Books, request: &OperationRequest| {
        books
            .registry
            .draft(request, &books.ledger)
            .err()
            .map(|e| e.code())
    };

    // An item already resolved, or not yet opened, cannot be resolved.
    assert_eq!(code(&books, &resolve(bank, 500)), Some(2014));

    let draft = books.registry.draft(&open(500), &books.ledger)?;
    books.post(draft)?;
    // The same item cannot be opened twice.
    assert_eq!(code(&books, &open(500)), Some(2015));
    // Only the whole item moves.
    assert_eq!(code(&books, &resolve(bank, 499)), Some(2014));
    assert_eq!(code(&books, &resolve(bank, 501)), Some(2014));
    // Never to revenue.
    assert_eq!(code(&books, &resolve(books.id("4010"), 500)), Some(2010));

    // More than 1010 holds is refused by the posting rules (INV-8).
    let other = books.id("9040:sbd-0001");
    let too_much = request(
        "bank_debit_unmatched.v1",
        &[("bank_debit_item", other), ("customer_funds", bank)],
        etb(books.balance("1010") + 1),
    );
    let draft = books.registry.draft(&too_much, &books.ledger)?;
    assert!(matches!(
        validate(draft, &books.ledger),
        Err(PostingError::InsufficientFunds { .. })
    ));

    // A wallet that no longer holds the money cannot repay it: the item
    // stays open.
    // Abebe spends all but 200 ETB of the wallet (a fee, for brevity).
    let wallet = books.id("2010:cust-abebe");
    let spend = EntryDraft {
        lines: vec![
            LineDraft {
                account: wallet,
                side: Side::Debit,
                amount: etb(99_800),
            },
            LineDraft {
                account: books.id("4010"),
                side: Side::Credit,
                amount: etb(99_800),
            },
        ],
        ..EntryDraft::default()
    };
    books.post(spend)?;
    let draft = books.registry.draft(&resolve(wallet, 500), &books.ledger)?;
    assert!(matches!(
        validate(draft, &books.ledger),
        Err(PostingError::InsufficientFunds { .. })
    ));
    assert_eq!(books.balance("9040:sbd-0003"), 500);

    // The bank gives it back: the item closes against 1010.
    let refund = EntryDraft {
        lines: vec![
            LineDraft {
                account: bank,
                side: Side::Debit,
                amount: etb(500),
            },
            LineDraft {
                account: books.id("9040:sbd-0003"),
                side: Side::Credit,
                amount: etb(500),
            },
        ],
        ..EntryDraft::default()
    };
    let draft = books.registry.draft(&resolve(bank, 500), &books.ledger)?;
    assert_eq!(draft.lines, refund.lines);
    books.post(draft)?;
    assert_eq!(books.balance("9040:sbd-0003"), 0);
    Ok(())
}

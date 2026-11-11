//! The worked entries from step 1.01 as posting fixtures (steps 1.06–1.07).
//!
//! Every journal table in `docs/accounting/entries/` is read straight from
//! the documentation, so the fixtures can never drift from it. The story is
//! replayed in order (OB, then E01–E18) through the reference ledger:
//! - each entry posts, with the total the docs print;
//! - each mutation of an entry, tried against the ledger as it stood just
//!   before that entry, is rejected with the expected error code;
//! - the invariants hold after every entry;
//! - the final balances equal `docs/accounting/trial-balance.md`.

mod common;

use std::collections::BTreeMap;

use common::{CHART, Fixture, fixtures, trial_balance};
use mezgeb_core::{
    AccountId, AccountStatus, Chart, Currency, EntryDraft, EntryId, LineDraft, MemLedger, Money,
    PostingError, check_invariants, validate,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Ids = BTreeMap<String, AccountId>;

/// A UUIDv7 ID built from a number.
fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

/// Opens every account the fixtures use in an empty ledger.
fn open_accounts(fixtures: &[Fixture]) -> Result<(MemLedger, Ids), Box<dyn std::error::Error>> {
    let chart = Chart::from_toml(CHART)?;
    let mut ledger = MemLedger::new();
    let mut ids = Ids::new();
    for line in fixtures.iter().flat_map(|f| &f.lines) {
        if ids.contains_key(&line.reference) {
            continue;
        }
        let id: AccountId = uuid7(ids.len() as u64).parse()?;
        ledger.open_account(id, &chart, &line.reference)?;
        ids.insert(line.reference.clone(), id);
    }
    Ok((ledger, ids))
}

fn draft(fixture: &Fixture, ids: &Ids) -> Result<EntryDraft, String> {
    let mut lines = Vec::new();
    for line in &fixture.lines {
        let account = *ids
            .get(&line.reference)
            .ok_or_else(|| format!("no id for {}", line.reference))?;
        lines.push(LineDraft {
            account,
            side: line.side,
            amount: line.amount,
        });
    }
    Ok(EntryDraft {
        lines,
        ..EntryDraft::default()
    })
}

/// A way to break a valid entry, and the error code it must produce.
struct Mutation {
    name: &'static str,
    code: u16,
    apply: fn(&mut EntryDraft, &mut MemLedger) -> Result<(), String>,
}

const MUTATIONS: [Mutation; 7] = [
    Mutation {
        name: "remove all but one line",
        code: 1001,
        apply: |draft, _| {
            draft.lines.truncate(1);
            Ok(())
        },
    },
    Mutation {
        name: "zero amount",
        code: 1004,
        apply: |draft, _| {
            let line = draft.lines.first_mut().ok_or("no lines")?;
            line.amount = Money::zero(line.amount.currency());
            Ok(())
        },
    },
    Mutation {
        name: "unknown account",
        code: 1005,
        apply: |draft, _| {
            let line = draft.lines.first_mut().ok_or("no lines")?;
            line.account = uuid7(999_999).parse().map_err(|e| format!("{e}"))?;
            Ok(())
        },
    },
    Mutation {
        name: "frozen account",
        code: 1006,
        apply: |draft, ledger| {
            let id = draft.lines.first().ok_or("no lines")?.account;
            ledger
                .set_status(id, AccountStatus::Frozen)
                .map_err(|e| e.to_string())
        },
    },
    Mutation {
        name: "wrong currency",
        code: 1007,
        apply: |draft, _| {
            let line = draft.lines.first_mut().ok_or("no lines")?;
            line.amount = Money::from_minor(line.amount.amount(), Currency::USD);
            Ok(())
        },
    },
    Mutation {
        name: "duplicate line",
        code: 1008,
        apply: |draft, _| {
            let first = draft.lines.first().ok_or("no lines")?.clone();
            draft.lines.push(first);
            Ok(())
        },
    },
    Mutation {
        name: "one santim more on one line",
        code: 1010,
        apply: |draft, _| {
            let line = draft.lines.first_mut().ok_or("no lines")?;
            line.amount = line
                .amount
                .checked_add(Money::from_minor(1, line.amount.currency()))
                .map_err(|e| e.to_string())?;
            Ok(())
        },
    },
];

#[test]
fn the_worked_story_replays_through_the_reference_ledger() -> TestResult {
    let fixtures = fixtures()?;
    // OB, E01–E18, plus the second tables of E04, E14 and E15 and the
    // second and third of E13.
    assert_eq!(fixtures.len(), 24, "journal tables found in the docs");

    let (mut ledger, ids) = open_accounts(&fixtures)?;
    let mut mutations_checked = 0;
    for (n, fixture) in fixtures.iter().enumerate() {
        // Every mutation is refused, against the ledger as it stands now.
        for mutation in &MUTATIONS {
            let mut broken = draft(fixture, &ids)?;
            let mut state = ledger.clone();
            (mutation.apply)(&mut broken, &mut state)?;
            let result = validate(broken, &state);
            assert_eq!(
                result.as_ref().map_err(PostingError::code).err(),
                Some(mutation.code),
                "{}, {}: got {:?}",
                fixture.label,
                mutation.name,
                result
            );
            mutations_checked += 1;
        }

        // The entry itself posts, with the total the docs print.
        let id: EntryId = uuid7(n as u64 + 1).parse()?;
        let entry = ledger
            .post(id, draft(fixture, &ids)?)
            .map_err(|e| format!("{}: {e}", fixture.label))?;
        let debits: i64 = entry
            .lines()
            .iter()
            .filter(|l| l.side() == mezgeb_core::Side::Debit)
            .map(|l| l.amount().amount())
            .sum();
        assert_eq!(debits, fixture.total.amount(), "{}: total", fixture.label);
        check_invariants(&ledger).map_err(|v| format!("after {}: {v}", fixture.label))?;
    }
    assert_eq!(mutations_checked, 24 * MUTATIONS.len());

    // The final balances are exactly the trial balance in the docs.
    let expected = trial_balance()?;
    assert_eq!(expected.len(), ids.len(), "trial balance rows");
    for (reference, balance) in expected {
        let id = ids
            .get(&reference)
            .ok_or_else(|| format!("{reference} is in the trial balance but never posted"))?;
        let account = ledger.account(*id).ok_or("account missing")?;
        assert_eq!(account.balance(), balance, "balance of {reference}");
    }
    Ok(())
}

#[test]
fn spending_more_than_a_wallet_holds_is_refused() -> TestResult {
    // E02 posted against an empty wallet: the money was never deposited.
    let fixtures = fixtures()?;
    let (ledger, ids) = open_accounts(&fixtures)?;
    let e02 = fixtures
        .iter()
        .find(|f| f.label == "E02 #1")
        .ok_or("E02 not found")?;
    assert_eq!(
        validate(draft(e02, &ids)?, &ledger),
        Err(PostingError::InsufficientFunds { line: 0 })
    );
    Ok(())
}

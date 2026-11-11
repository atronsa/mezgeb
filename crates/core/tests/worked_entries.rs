//! The worked entries from step 1.01 as posting fixtures (step 1.06).
//!
//! Every journal table in `docs/accounting/entries/` is read straight from
//! the documentation, so the fixtures can never drift from it. Each one must
//! validate, and each mutation of it must be rejected with the expected
//! error code.

mod common;

use std::collections::BTreeMap;

use common::{CHART, Fixture, fixtures};
use mezgeb_core::{
    Account, AccountId, AccountStatus, Chart, Currency, EntryDraft, LineDraft, Money, PostingError,
    validate,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type View = BTreeMap<AccountId, Account>;

/// A UUIDv7 account ID built from a number.
fn account_id(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    format!("00000000-0000-7000-8000-{n:012x}").parse()
}

/// Opens every account the fixtures use, and returns the accounts plus the
/// reference → ID map.
fn open_accounts(
    fixtures: &[Fixture],
) -> Result<(View, BTreeMap<String, AccountId>), Box<dyn std::error::Error>> {
    let chart = Chart::from_toml(CHART)?;
    let mut view = View::new();
    let mut ids = BTreeMap::new();
    for line in fixtures.iter().flat_map(|f| &f.lines) {
        if ids.contains_key(&line.reference) {
            continue;
        }
        let id = account_id(ids.len() as u64)?;
        view.insert(id, Account::open(id, &chart, &line.reference)?);
        ids.insert(line.reference.clone(), id);
    }
    Ok((view, ids))
}

fn draft(fixture: &Fixture, ids: &BTreeMap<String, AccountId>) -> Result<EntryDraft, String> {
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

#[test]
fn every_worked_entry_validates() -> TestResult {
    let fixtures = fixtures()?;
    // OB, E01–E18, plus the second tables of E04, E14 and E15 and the
    // second and third of E13.
    assert_eq!(fixtures.len(), 24, "journal tables found in the docs");

    let (view, ids) = open_accounts(&fixtures)?;
    for fixture in &fixtures {
        let entry = validate(draft(fixture, &ids)?, &view)
            .map_err(|e| format!("{}: {e}", fixture.label))?;
        // The total the docs print matches what the ledger computes.
        assert_eq!(
            entry.totals(),
            BTreeMap::from([(Currency::ETB, fixture.total.amount())]),
            "{}: total",
            fixture.label
        );
    }
    Ok(())
}

/// A way to break a valid entry, and the error code it must produce.
struct Mutation {
    name: &'static str,
    code: u16,
    apply: fn(&mut EntryDraft, &mut View) -> Result<(), String>,
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
            line.account = account_id(999_999).map_err(|e| e.to_string())?;
            Ok(())
        },
    },
    Mutation {
        name: "frozen account",
        code: 1006,
        apply: |draft, view| {
            let id = draft.lines.first().ok_or("no lines")?.account;
            let account = view.get_mut(&id).ok_or("account missing")?;
            account
                .set_status(AccountStatus::Frozen)
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
fn every_mutated_worked_entry_is_rejected() -> TestResult {
    let fixtures = fixtures()?;
    let (view, ids) = open_accounts(&fixtures)?;
    let mut checked = 0;
    for fixture in &fixtures {
        for mutation in &MUTATIONS {
            let mut draft = draft(fixture, &ids)?;
            let mut view = view.clone();
            (mutation.apply)(&mut draft, &mut view)?;
            let result = validate(draft, &view);
            assert_eq!(
                result.as_ref().map_err(PostingError::code).err(),
                Some(mutation.code),
                "{}, {}: got {:?}",
                fixture.label,
                mutation.name,
                result
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 24 * MUTATIONS.len());
    Ok(())
}

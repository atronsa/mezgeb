//! Tests of the reference model: unit tests for `MemLedger` and the
//! invariant checker, the state-machine property test, and the planted-bug
//! test (step 1.07).

use std::cell::Cell;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestError, TestRunner};

use super::invariants::{
    check_balances, check_entries, check_global_balance, check_reversals, check_running_totals,
};
use super::*;
use crate::account::{AccountStatus, StatusError};
use crate::chart::{Chart, ChartError, Side};
use crate::currency::Currency;
use crate::entry::EntryDraft;
use crate::error::PostingError;
use crate::ids::{AccountId, EntryId};
use crate::money::{Money, PositiveAmount};
use crate::validate::test_support::account_id;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHART: &str = r#"
name = "Model test chart"
currency = "ETB"

[[accounts]]
code = 1010
name = "Bank"
type = "asset"
kind = "system"
status = "active"

[[accounts]]
code = 2010
name = "Customer wallets"
type = "liability"
kind = "control"
prefix = "cust"
status = "active"

[[accounts]]
code = 3010
name = "Capital"
type = "equity"
kind = "system"
status = "active"

[[accounts]]
code = 4010
name = "Fee revenue"
type = "revenue"
kind = "system"
status = "active"

[[accounts]]
code = 5010
name = "Compensation"
type = "expense"
kind = "system"
status = "active"

[[accounts]]
code = 9010
name = "Suspense"
type = "liability"
kind = "system"
allow_negative = true
status = "active"
"#;

/// The accounts the state machine works with, by slot.
const SLOTS: [&str; 8] = [
    "1010",
    "2010:cust-a",
    "2010:cust-b",
    "2010:cust-c",
    "3010",
    "4010",
    "5010",
    "9010",
];

fn chart() -> Result<Chart, ChartError> {
    Chart::from_toml(CHART)
}

fn slot(n: usize) -> AccountId {
    account_id(n as u64)
}

fn entry_id(n: u64) -> EntryId {
    let mut bytes = [0u8; 16];
    bytes[8..].copy_from_slice(&n.to_be_bytes());
    bytes[6] = 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    EntryId::from_uuid(uuid::Uuid::from_bytes(bytes))
        .unwrap_or_else(|_| unreachable!("version and variant bits are set"))
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// A ledger with the bank and two wallets open, and 1,000 santim deposited
/// to wallet a (entry 1).
fn funded() -> Result<MemLedger, Box<dyn std::error::Error>> {
    let chart = chart()?;
    let mut ledger = MemLedger::new();
    for (n, reference) in SLOTS.iter().enumerate().take(3) {
        ledger.open_account(slot(n), &chart, reference)?;
    }
    ledger.post(
        entry_id(1),
        EntryDraft::default()
            .debit(slot(0), etb(1_000))
            .credit(slot(1), etb(1_000)),
    )?;
    Ok(ledger)
}

// ---------------------------------------------------------------------------
// MemLedger operations
// ---------------------------------------------------------------------------

#[test]
fn opens_accounts_once() -> TestResult {
    let chart = chart()?;
    let mut ledger = MemLedger::new();
    let account = ledger.open_account(slot(0), &chart, "1010")?;
    assert_eq!(account.reference(), "1010");
    assert_eq!(
        ledger.open_account(slot(0), &chart, "4010").err(),
        Some(LedgerError::DuplicateAccount(slot(0)))
    );
    assert_eq!(
        ledger.open_account(slot(1), &chart, "2010").err(),
        Some(LedgerError::Chart(ChartError::SubAccountRequired(2010)))
    );
    assert_eq!(ledger.accounts().count(), 1);
    Ok(())
}

#[test]
fn posts_and_records_running_totals() -> TestResult {
    let mut ledger = funded()?;
    let entry = ledger.post(
        entry_id(2),
        EntryDraft::default()
            .debit(slot(1), etb(300))
            .credit(slot(2), etb(300)),
    )?;
    assert_eq!(entry.id(), entry_id(2));
    assert_eq!(entry.sequence(), 2);
    assert_eq!(entry.reverses(), None);
    assert!(entry.metadata().is_empty());
    assert_eq!(entry.event_time(), None);
    let line = &entry.lines()[0];
    assert_eq!(line.account(), slot(1));
    assert_eq!(line.side(), Side::Debit);
    assert_eq!(line.amount(), PositiveAmount::new(etb(300))?);
    assert_eq!((line.debits_after(), line.credits_after()), (300, 1_000));

    assert_eq!(ledger.account(slot(1)).map(|a| a.balance()), Some(etb(700)));
    assert_eq!(ledger.account(slot(2)).map(|a| a.balance()), Some(etb(300)));
    assert_eq!(ledger.history(slot(1)).len(), 2);
    assert_eq!(ledger.entries().len(), 2);
    check_invariants(&ledger)?;
    Ok(())
}

#[test]
fn refused_posts_change_nothing() -> TestResult {
    let mut ledger = funded()?;
    let before = ledger.clone();
    let overdraw = EntryDraft::default()
        .debit(slot(1), etb(1_001))
        .credit(slot(2), etb(1_001));
    assert_eq!(
        ledger.post(entry_id(2), overdraw).err(),
        Some(LedgerError::Posting(PostingError::InsufficientFunds {
            line: 0
        }))
    );
    let again = EntryDraft::default()
        .debit(slot(0), etb(1))
        .credit(slot(1), etb(1));
    assert_eq!(
        ledger.post(entry_id(1), again).err(),
        Some(LedgerError::DuplicateEntry(entry_id(1)))
    );
    assert_eq!(ledger, before);
    Ok(())
}

#[test]
fn reverses_exactly_once() -> TestResult {
    let mut ledger = funded()?;
    let reversal = ledger.reverse(entry_id(1), entry_id(2))?;
    assert_eq!(reversal.reverses(), Some(entry_id(1)));
    assert_eq!(reversal.lines()[0].side(), Side::Credit);
    assert_eq!(ledger.reversal_of(entry_id(1)), Some(entry_id(2)));
    assert_eq!(ledger.account(slot(1)).map(|a| a.balance()), Some(etb(0)));

    assert_eq!(
        ledger.reverse(entry_id(1), entry_id(3)).err(),
        Some(LedgerError::AlreadyReversed(entry_id(1)))
    );
    assert_eq!(
        ledger.reverse(entry_id(2), entry_id(3)).err(),
        Some(LedgerError::CannotReverseReversal(entry_id(2)))
    );
    assert_eq!(
        ledger.reverse(entry_id(9), entry_id(3)).err(),
        Some(LedgerError::EntryNotFound(entry_id(9)))
    );
    assert_eq!(
        ledger.reverse(entry_id(1), entry_id(2)).err(),
        Some(LedgerError::DuplicateEntry(entry_id(2)))
    );
    check_invariants(&ledger)?;
    Ok(())
}

#[test]
fn a_reversal_of_spent_money_is_refused() -> TestResult {
    // Like E13: the deposit was spent before anyone noticed it was wrong.
    let mut ledger = funded()?;
    ledger.post(
        entry_id(2),
        EntryDraft::default()
            .debit(slot(1), etb(600))
            .credit(slot(2), etb(600)),
    )?;
    let before = ledger.clone();
    assert_eq!(
        ledger.reverse(entry_id(1), entry_id(3)).err(),
        Some(LedgerError::Posting(PostingError::InsufficientFunds {
            line: 1
        }))
    );
    assert_eq!(ledger, before);
    assert_eq!(ledger.reversal_of(entry_id(1)), None);
    Ok(())
}

#[test]
fn status_changes() -> TestResult {
    let mut ledger = funded()?;
    ledger.set_status(slot(2), AccountStatus::Closed)?;
    assert_eq!(
        ledger.set_status(slot(1), AccountStatus::Closed).err(),
        Some(LedgerError::Status(StatusError::BalanceNotZero))
    );
    assert_eq!(
        ledger.set_status(slot(7), AccountStatus::Frozen).err(),
        Some(LedgerError::AccountNotFound(slot(7)))
    );
    let to_closed = EntryDraft::default()
        .debit(slot(1), etb(1))
        .credit(slot(2), etb(1));
    assert_eq!(
        ledger.post(entry_id(2), to_closed).err(),
        Some(LedgerError::Posting(PostingError::AccountNotOpen {
            line: 1,
            status: AccountStatus::Closed
        }))
    );
    Ok(())
}

#[test]
fn errors_have_messages() {
    let id = slot(0);
    let entry = entry_id(1);
    let cases = [
        (
            LedgerError::Chart(ChartError::UnknownAccount(3010)),
            "account 3010 is not in the chart".to_owned(),
        ),
        (
            LedgerError::DuplicateAccount(id),
            format!("account {id} already exists"),
        ),
        (
            LedgerError::AccountNotFound(id),
            format!("account {id} not found"),
        ),
        (
            LedgerError::Status(StatusError::AccountClosed),
            "a closed account cannot change status".to_owned(),
        ),
        (
            LedgerError::DuplicateEntry(entry),
            format!("entry {entry} already exists"),
        ),
        (
            LedgerError::Posting(PostingError::TooFewLines),
            "[1001] an entry needs at least 2 lines".to_owned(),
        ),
        (
            LedgerError::EntryNotFound(entry),
            format!("entry {entry} not found"),
        ),
        (
            LedgerError::AlreadyReversed(entry),
            format!("entry {entry} is already reversed"),
        ),
        (
            LedgerError::CannotReverseReversal(entry),
            format!("entry {entry} is a reversal and cannot be reversed"),
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
    let violation = InvariantViolation {
        invariant: "INV-2",
        detail: "x".into(),
    };
    assert_eq!(violation.to_string(), "INV-2 violated: x");
}

// ---------------------------------------------------------------------------
// The invariant checker catches each kind of corruption
// ---------------------------------------------------------------------------

fn violated(result: Result<(), InvariantViolation>) -> Option<&'static str> {
    result.err().map(|v| v.invariant)
}

/// The funded ledger plus a reversal, so every check has data to look at.
fn sample() -> Result<MemLedger, Box<dyn std::error::Error>> {
    let mut ledger = funded()?;
    ledger.post(
        entry_id(2),
        EntryDraft::default()
            .debit(slot(1), etb(100))
            .credit(slot(2), etb(100)),
    )?;
    ledger.reverse(entry_id(2), entry_id(3))?;
    check_invariants(&ledger)?;
    Ok(ledger)
}

#[test]
fn detects_entry_corruption() -> TestResult {
    let mut l = sample()?;
    l.entries[1].sequence = 1;
    assert_eq!(violated(check_entries(&l)), Some("INV-12"));

    let mut l = sample()?;
    l.entries[0].lines.truncate(1);
    assert_eq!(violated(check_entries(&l)), Some("INV-1"));

    let mut l = sample()?;
    l.entries[0].lines[0].amount = PositiveAmount::new(etb(999))?;
    assert_eq!(violated(check_entries(&l)), Some("INV-1"));

    let mut l = sample()?;
    l.entries[0].lines[0].account = slot(7);
    assert_eq!(violated(check_entries(&l)), Some("INV-24"));

    let mut l = sample()?;
    l.entries[0].lines[0].amount = PositiveAmount::new(Money::from_minor(1_000, Currency::USD))?;
    assert_eq!(violated(check_entries(&l)), Some("INV-3"));
    Ok(())
}

#[test]
fn detects_total_corruption() -> TestResult {
    let mut l = sample()?;
    l.entries[0].lines[0].debits_after += 1;
    assert_eq!(violated(check_running_totals(&l)), Some("INV-6"));

    let mut l = sample()?;
    if let Some(account) = l.accounts.get_mut(&slot(1)) {
        account.record(Side::Credit, 5);
    }
    assert_eq!(violated(check_running_totals(&l)), Some("INV-5"));
    assert_eq!(violated(check_global_balance(&l)), Some("INV-2"));

    let mut l = sample()?;
    l.entries[0].lines[0].amount = PositiveAmount::new(etb(i64::MAX))?;
    l.entries[0].lines[0].debits_after = i64::MAX;
    l.entries[1].lines[0].account = slot(0);
    l.entries[1].lines[0].side = Side::Debit;
    assert_eq!(violated(check_running_totals(&l)), Some("INV-7"));
    Ok(())
}

#[test]
fn detects_balance_corruption() -> TestResult {
    let mut l = sample()?;
    if let Some(account) = l.accounts.get_mut(&slot(2)) {
        account.record(Side::Debit, 5_000);
    }
    assert_eq!(violated(check_balances(&l)), Some("INV-8"));

    let mut l = sample()?;
    l.set_status(slot(2), AccountStatus::Closed)?;
    if let Some(account) = l.accounts.get_mut(&slot(2)) {
        account.record(Side::Credit, 1);
    }
    assert_eq!(violated(check_balances(&l)), Some("INV-8"));
    Ok(())
}

#[test]
fn detects_reversal_corruption() -> TestResult {
    let mut l = sample()?;
    l.entries[2].lines[0].side = Side::Debit;
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));

    let mut l = sample()?;
    l.entries[1].reverses = Some(entry_id(1));
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));

    let mut l = sample()?;
    l.entries[0].reverses = Some(entry_id(3));
    l.entries[2].reverses = Some(entry_id(1));
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));

    let mut l = sample()?;
    l.entries[2].reverses = Some(entry_id(99));
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));

    let mut l = sample()?;
    l.entries[0].reverses = Some(entry_id(2));
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));

    let mut l = sample()?;
    let mut twice = l.entries[2].clone();
    twice.id = entry_id(4);
    twice.sequence = 4;
    l.entries.push(twice);
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));

    let mut l = sample()?;
    l.reversed_by.clear();
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));

    let mut l = sample()?;
    l.entries[2].reverses = Some(entry_id(1));
    assert_eq!(violated(check_reversals(&l)), Some("INV-14"));
    Ok(())
}

// ---------------------------------------------------------------------------
// State machine: random operation sequences, invariants after every step
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Open(usize),
    Transfer {
        debit: usize,
        credit: usize,
        amount: i64,
    },
    Split {
        debit: usize,
        credit: (usize, usize),
        amounts: (i64, i64),
    },
    /// Arbitrary lines, usually refused: checks that refusals change nothing.
    Raw(Vec<(usize, Side, i64)>),
    /// Reverse the entry at this index (modulo the number of entries).
    Reverse(usize),
    Status(usize, AccountStatus),
}

fn op() -> impl Strategy<Value = Op> {
    let slot = || 0..SLOTS.len();
    let side = prop_oneof![Just(Side::Debit), Just(Side::Credit)];
    prop_oneof![
        2 => slot().prop_map(Op::Open),
        5 => (slot(), slot(), 1..1_000i64)
            .prop_map(|(debit, credit, amount)| Op::Transfer { debit, credit, amount }),
        3 => (slot(), slot(), slot(), 1..1_000i64, 1..1_000i64).prop_map(|(d, c1, c2, a1, a2)| {
            Op::Split { debit: d, credit: (c1, c2), amounts: (a1, a2) }
        }),
        1 => proptest::collection::vec((slot(), side, -5..1_000i64), 0..5).prop_map(Op::Raw),
        2 => any::<usize>().prop_map(Op::Reverse),
        1 => (slot(), proptest::sample::select(AccountStatus::ALL.as_slice()))
            .prop_map(|(s, status)| Op::Status(s, status)),
    ]
}

/// Runs the operations and checks every invariant after each one. A refused
/// operation must leave the ledger exactly as it was.
fn run(ops: &[Op], planted_bug: bool) -> Result<(), TestCaseError> {
    let chart = chart().map_err(|e| TestCaseError::fail(e.to_string()))?;
    let mut ledger = MemLedger {
        planted_bug,
        ..MemLedger::default()
    };
    let mut next_entry = 0;
    for (step, op) in ops.iter().enumerate() {
        let before = ledger.clone();
        next_entry += 1;
        let id = entry_id(next_entry);
        let result = match op {
            Op::Open(s) => ledger.open_account(slot(*s), &chart, SLOTS[*s]).map(|_| ()),
            Op::Transfer {
                debit,
                credit,
                amount,
            } => {
                let draft = EntryDraft::default()
                    .debit(slot(*debit), etb(*amount))
                    .credit(slot(*credit), etb(*amount));
                ledger.post(id, draft).map(|_| ())
            }
            Op::Split {
                debit,
                credit,
                amounts,
            } => {
                let draft = EntryDraft::default()
                    .debit(slot(*debit), etb(amounts.0 + amounts.1))
                    .credit(slot(credit.0), etb(amounts.0))
                    .credit(slot(credit.1), etb(amounts.1));
                ledger.post(id, draft).map(|_| ())
            }
            Op::Raw(lines) => {
                let mut draft = EntryDraft::default();
                for (s, side, amount) in lines {
                    draft = match side {
                        Side::Debit => draft.debit(slot(*s), etb(*amount)),
                        Side::Credit => draft.credit(slot(*s), etb(*amount)),
                    };
                }
                ledger.post(id, draft).map(|_| ())
            }
            Op::Reverse(i) => match ledger.entries().len() {
                0 => Ok(()),
                n => {
                    let original = ledger.entries()[i % n].id();
                    ledger.reverse(original, id).map(|_| ())
                }
            },
            Op::Status(s, status) => ledger.set_status(slot(*s), *status),
        };
        if result.is_err() {
            prop_assert_eq!(
                &ledger,
                &before,
                "step {}: a refused {:?} changed the ledger",
                step,
                op
            );
        }
        check_invariants(&ledger)
            .map_err(|v| TestCaseError::fail(format!("step {step}, after {op:?}: {v}")))?;
    }
    Ok(())
}

proptest! {
    /// The step 1.07 exit test runs this with `PROPTEST_CASES=10000`.
    #[test]
    fn invariants_hold_after_every_operation(ops in proptest::collection::vec(op(), 200)) {
        run(&ops, false)?;
    }
}

/// Plants a bug (the last line of every entry does not update its account's
/// totals) and checks that the state-machine test catches it within 1,000
/// cases and shrinks it to a minimal sequence.
#[test]
fn planted_bug_is_caught_and_shrunk() -> Result<(), String> {
    let mut runner = TestRunner::new(Config {
        cases: 1_000,
        failure_persistence: None,
        ..Config::default()
    });
    let cases_run = Cell::new(0u32);
    let first_failure = Cell::new(None);
    let result = runner.run(&proptest::collection::vec(op(), 1..=200), |ops| {
        cases_run.set(cases_run.get() + 1);
        let outcome = run(&ops, true);
        if outcome.is_err() && first_failure.get().is_none() {
            first_failure.set(Some(cases_run.get()));
        }
        outcome
    });
    match result {
        Err(TestError::Fail(reason, minimal)) => {
            eprintln!(
                "planted bug caught on case {:?}; shrunk to {} operations: {minimal:?}\nreason: {reason}",
                first_failure.get(),
                minimal.len()
            );
            if minimal.len() > 3 {
                return Err(format!("shrinking left {} operations", minimal.len()));
            }
            Ok(())
        }
        other => Err(format!("planted bug was not caught: {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// Additions for step 1.09
// ---------------------------------------------------------------------------

#[test]
fn one_account_per_chart_reference() -> TestResult {
    let chart = chart()?;
    let mut ledger = MemLedger::new();
    ledger.open_account(slot(0), &chart, "1010")?;
    assert_eq!(
        ledger.open_account(slot(1), &chart, "1010").err(),
        Some(LedgerError::DuplicateReference("1010".into()))
    );
    assert_eq!(
        LedgerError::DuplicateReference("1010".into()).to_string(),
        "an account for 1010 already exists"
    );
    assert_eq!(
        LedgerError::Storage("disk full".into()).to_string(),
        "storage error: disk full"
    );
    Ok(())
}

/// Found by the step 1.09 differential test: an entry that debits and
/// credits the same account nets to zero and must post, even on an account
/// that cannot go negative.
#[test]
fn the_same_account_on_both_sides_posts() -> TestResult {
    let chart = chart()?;
    let mut ledger = MemLedger::new();
    ledger.open_account(slot(4), &chart, "3010")?;
    let draft = EntryDraft::default()
        .debit(slot(4), etb(1))
        .credit(slot(4), etb(1));
    ledger.post(entry_id(1), draft)?;
    check_invariants(&ledger)?;
    Ok(())
}

#[test]
fn the_ledger_trait_matches_the_inherent_methods() -> TestResult {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    // MemLedger's futures are always ready, so one poll completes them.
    fn now<T>(future: impl Future<Output = T>) -> Result<T, &'static str> {
        let mut context = Context::from_waker(Waker::noop());
        match pin!(future).poll(&mut context) {
            Poll::Ready(value) => Ok(value),
            Poll::Pending => Err("future was not ready"),
        }
    }

    let chart = chart()?;
    let mut ledger = MemLedger::new();
    let bank = now(Ledger::open_account(&mut ledger, slot(0), &chart, "1010"))??;
    now(Ledger::open_account(
        &mut ledger,
        slot(1),
        &chart,
        "2010:cust-a",
    ))??;
    let draft = EntryDraft::default()
        .debit(slot(0), etb(10))
        .credit(slot(1), etb(10));
    let posted = now(Ledger::post(&mut ledger, entry_id(1), draft))??;
    let reversal = now(Ledger::reverse(&mut ledger, entry_id(1), entry_id(2)))??;
    now(Ledger::set_status(
        &mut ledger,
        slot(1),
        AccountStatus::Frozen,
    ))??;

    assert_eq!(bank.reference(), "1010");
    assert_eq!(reversal.reverses(), Some(posted.id()));
    assert_eq!(
        now(Ledger::get_account(&mut ledger, slot(1)))??.map(|a| a.status()),
        Some(AccountStatus::Frozen)
    );
    assert_eq!(now(Ledger::all_accounts(&mut ledger))??.len(), 2);
    assert_eq!(
        now(Ledger::all_entries(&mut ledger))??,
        ledger.entries().to_vec()
    );
    let rebuilt = PostedEntry::new(
        posted.id(),
        posted.sequence(),
        posted.lines().to_vec(),
        posted.reverses(),
        posted.metadata().clone(),
        posted.event_time(),
    );
    assert_eq!(rebuilt, posted);
    let line = &posted.lines()[0];
    assert_eq!(
        PostedLine::new(
            line.account(),
            line.side(),
            line.amount(),
            line.debits_after(),
            line.credits_after()
        ),
        *line
    );
    Ok(())
}

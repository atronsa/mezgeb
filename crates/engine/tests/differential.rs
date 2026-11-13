//! Differential test (step 1.09): the same random operation sequence runs on
//! the reference ledger (`MemLedger`) and on PostgreSQL (`PgStore`). Every
//! result must be identical, and so must the final accounts and history.
//!
//! The number of sequences comes from `MEZGEB_DIFF_CASES` (default 16); the
//! step 1.09 acceptance run uses 1,000.

mod support;

use mezgeb_core::{
    Account, AccountId, AccountStatus, ActorId, Authorization, Chart, ClientId, Currency,
    EntryDraft, EntryId, Hold, HoldId, Holds, Ledger, LedgerError, MemLedger, Money,
    PositiveAmount, PostedEntry, ReasonCode, Side, Timestamp,
};
use mezgeb_engine::{PgStore, integrity_report};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestError, TestRunner};
use support::{Server, TestDb, uuid7};

const CHART: &str = r#"
name = "Differential test chart"
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

[[accounts]]
code = 2020
name = "Held funds"
type = "liability"
kind = "control"
prefix = "hold"
status = "active"
"#;

/// The accounts the operations work with, by slot.
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
    /// Arbitrary lines, usually refused.
    Raw(Vec<(usize, Side, i64)>),
    /// Reverse the entry at this index (modulo the number of entries).
    Reverse(usize),
    /// An adjustment with one of `REASONS` (one of them a reversal reason,
    /// so refused), approved by someone else or by the requester (refused).
    Adjust {
        debit: usize,
        credit: usize,
        amount: i64,
        reason: usize,
        self_approved: bool,
    },
    /// Place a hold on a slot's account, in a freshly opened held account.
    PlaceHold {
        account: usize,
        amount: i64,
        ttl: i64,
    },
    /// Capture part or all of the hold at this index.
    Capture {
        hold: usize,
        destination: usize,
        amount: i64,
    },
    ReleaseHold(usize),
    ExpireHold(usize),
    /// Let time pass.
    Tick(i64),
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
        2 => (slot(), slot(), 1..1_000i64, 0..REASONS.len(), proptest::bool::weighted(0.2))
            .prop_map(|(debit, credit, amount, reason, self_approved)| Op::Adjust {
                debit, credit, amount, reason, self_approved,
            }),
        3 => (slot(), 1..600i64, 1..5i64)
            .prop_map(|(account, amount, ttl)| Op::PlaceHold { account, amount, ttl }),
        3 => (any::<usize>(), slot(), 1..700i64)
            .prop_map(|(hold, destination, amount)| Op::Capture { hold, destination, amount }),
        1 => any::<usize>().prop_map(Op::ReleaseHold),
        1 => any::<usize>().prop_map(Op::ExpireHold),
        1 => (1..3i64).prop_map(Op::Tick),
        1 => (slot(), proptest::sample::select(AccountStatus::ALL.as_slice()))
            .prop_map(|(s, status)| Op::Status(s, status)),
    ]
}

const REASONS: [&str; 3] = ["goodwill_credit", "compensation", "booking_error"];

fn authorization(reason: &str, approver: Option<&str>) -> Result<Authorization, String> {
    let name = |e: mezgeb_core::NameError| e.to_string();
    Ok(Authorization {
        reason: ReasonCode::new(reason).map_err(name)?,
        requested_by: ActorId::new("ops:1").map_err(name)?,
        approved_by: approver.map(ActorId::new).transpose().map_err(name)?,
    })
}

fn slot(n: usize) -> Result<AccountId, String> {
    uuid7(n as u64).parse().map_err(|e| format!("{e}"))
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// What an operation returned, comparable across ledgers.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    Opened(Account),
    Posted(PostedEntry),
    Held(Hold),
}

/// What hold operations need to know: the current time, the hold to act
/// on (picked from the reference ledger's holds), and the IDs for a new
/// hold and its held account.
struct HoldContext {
    now: i64,
    target: Option<HoldId>,
    new_hold: HoldId,
    new_held: AccountId,
    new_held_reference: String,
}

/// Applies one operation to a ledger.
async fn apply<L: Ledger + Holds>(
    ledger: &mut L,
    chart: &Chart,
    op: &Op,
    id: EntryId,
    reverse_target: Option<EntryId>,
    holds: &HoldContext,
) -> Result<Result<Outcome, LedgerError>, String> {
    let positive = |amount: i64| PositiveAmount::new(etb(amount)).map_err(|e| e.to_string());
    let now = Timestamp::from_micros(holds.now);
    let line = |s: usize, side: Side, amount: i64| -> Result<_, String> {
        Ok(mezgeb_core::LineDraft {
            account: slot(s)?,
            side,
            amount: etb(amount),
        })
    };
    Ok(match op {
        Op::Open(s) => ledger
            .open_account(slot(*s)?, chart, SLOTS[*s])
            .await
            .map(Outcome::Opened),
        Op::Transfer {
            debit,
            credit,
            amount,
        } => {
            let draft = EntryDraft {
                lines: vec![
                    line(*debit, Side::Debit, *amount)?,
                    line(*credit, Side::Credit, *amount)?,
                ],
                ..EntryDraft::default()
            };
            ledger.post(id, draft).await.map(Outcome::Posted)
        }
        Op::Split {
            debit,
            credit,
            amounts,
        } => {
            let draft = EntryDraft {
                lines: vec![
                    line(*debit, Side::Debit, amounts.0 + amounts.1)?,
                    line(credit.0, Side::Credit, amounts.0)?,
                    line(credit.1, Side::Credit, amounts.1)?,
                ],
                ..EntryDraft::default()
            };
            ledger.post(id, draft).await.map(Outcome::Posted)
        }
        Op::Raw(lines) => {
            let mut draft = EntryDraft::default();
            for (s, side, amount) in lines {
                draft.lines.push(line(*s, *side, *amount)?);
            }
            ledger.post(id, draft).await.map(Outcome::Posted)
        }
        Op::Reverse(_) => match reverse_target {
            Some(original) => ledger
                .reverse(original, id, authorization("booking_error", None)?)
                .await
                .map(Outcome::Posted),
            None => Ok(Outcome::Done),
        },
        Op::Adjust {
            debit,
            credit,
            amount,
            reason,
            self_approved,
        } => {
            let draft = EntryDraft {
                lines: vec![
                    line(*debit, Side::Debit, *amount)?,
                    line(*credit, Side::Credit, *amount)?,
                ],
                ..EntryDraft::default()
            };
            let approver = if *self_approved { "ops:1" } else { "lead:2" };
            ledger
                .post_adjustment(id, draft, authorization(REASONS[*reason], Some(approver))?)
                .await
                .map(Outcome::Posted)
        }
        Op::PlaceHold {
            account,
            amount,
            ttl,
        } => {
            // The held account opens on both ledgers, or the test is wrong.
            ledger
                .open_account(holds.new_held, chart, &holds.new_held_reference)
                .await
                .map_err(|e| format!("opening a held account: {e}"))?;
            ledger
                .place_hold(
                    holds.new_hold,
                    id,
                    slot(*account)?,
                    holds.new_held,
                    positive(*amount)?,
                    Timestamp::from_micros(holds.now + ttl),
                )
                .await
                .map(Outcome::Held)
        }
        Op::Capture {
            destination,
            amount,
            ..
        } => match holds.target {
            Some(hold) => ledger
                .capture_hold(hold, id, slot(*destination)?, positive(*amount)?, now)
                .await
                .map(Outcome::Held),
            None => Ok(Outcome::Done),
        },
        Op::ReleaseHold(_) => match holds.target {
            Some(hold) => ledger.release_hold(hold, id).await.map(Outcome::Held),
            None => Ok(Outcome::Done),
        },
        Op::ExpireHold(_) => match holds.target {
            Some(hold) => ledger.expire_hold(hold, id, now).await.map(Outcome::Held),
            None => Ok(Outcome::Done),
        },
        Op::Tick(_) => Ok(Outcome::Done),
        Op::Status(s, status) => ledger
            .set_status(slot(*s)?, *status)
            .await
            .map(|()| Outcome::Done),
    })
}

/// Runs one sequence on both ledgers and compares everything.
async fn run_case(server: &Server, template: &TestDb, ops: &[Op]) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| e.to_string();
    let chart = Chart::from_toml(CHART).map_err(|e| fail(&e))?;
    let db = server.copy_of(template).await.map_err(|e| fail(&e))?;
    let client: ClientId = uuid7(0xc11e).parse().map_err(|e| fail(&e))?;
    let mut store = PgStore::new(db.app_pool().await.map_err(|e| fail(&e))?, client);
    let mut oracle = MemLedger::new();

    let result = async {
        let mut now = 0i64;
        for (step, op) in ops.iter().enumerate() {
            let id: EntryId = uuid7(10_000 + step as u64).parse().map_err(|e| fail(&e))?;
            let target = match op {
                Op::Reverse(i) => {
                    let entries = oracle.entries();
                    (!entries.is_empty()).then(|| entries[i % entries.len()].id())
                }
                _ => None,
            };
            if let Op::Tick(ticks) = op {
                now += ticks;
            }
            let existing: Vec<HoldId> = oracle.holds().map(|h| h.id()).collect();
            let hold_target = match op {
                Op::Capture { hold: i, .. } | Op::ReleaseHold(i) | Op::ExpireHold(i) => {
                    (!existing.is_empty()).then(|| existing[i % existing.len()])
                }
                _ => None,
            };
            let holds = HoldContext {
                now,
                target: hold_target,
                new_hold: uuid7(50_000 + step as u64).parse().map_err(|e| fail(&e))?,
                new_held: uuid7(60_000 + step as u64).parse().map_err(|e| fail(&e))?,
                new_held_reference: format!("2020:hold-{step}"),
            };
            let expected = apply(&mut oracle, &chart, op, id, target, &holds).await?;
            let actual = apply(&mut store, &chart, op, id, target, &holds).await?;
            if actual != expected {
                return Err(format!(
                    "step {step}, {op:?}:\n  oracle:     {expected:?}\n  postgresql: {actual:?}"
                ));
            }
        }
        let accounts = (
            Ledger::all_accounts(&mut oracle).await,
            store.all_accounts().await,
        );
        if accounts.0 != accounts.1 {
            return Err(format!("final accounts differ: {accounts:?}"));
        }
        let entries = (
            Ledger::all_entries(&mut oracle).await,
            store.all_entries().await,
        );
        if entries.0 != entries.1 {
            return Err(format!("final history differs: {entries:?}"));
        }
        let holds = (
            Holds::all_holds(&mut oracle).await,
            Holds::all_holds(&mut store).await,
        );
        if holds.0 != holds.1 {
            return Err(format!("final holds differ: {holds:?}"));
        }
        let mut conn = db.app().await.map_err(|e| fail(&e))?;
        let report = integrity_report(&mut conn).await.map_err(|e| fail(&e))?;
        if !report.is_clean() {
            return Err(format!("integrity: {report:?}"));
        }
        Ok(())
    }
    .await;

    drop(store);
    db.drop().await.map_err(|e| fail(&e))?;
    result
}

#[test]
fn postgresql_matches_the_reference_ledger() -> Result<(), Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let cases = std::env::var("MEZGEB_DIFF_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(16);
    let runtime = tokio::runtime::Runtime::new()?;
    let template = runtime.block_on(server.template())?;

    let mut runner = TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    });
    let result = runner.run(&proptest::collection::vec(op(), 1..=40), |ops| {
        runtime
            .block_on(run_case(&server, &template, &ops))
            .map_err(TestCaseError::fail)
    });
    runtime.block_on(template.drop())?;
    match result {
        Ok(()) => {
            eprintln!("differential test: {cases} sequences matched");
            Ok(())
        }
        Err(TestError::Fail(reason, minimal)) => {
            Err(format!("{reason}\nminimal sequence: {minimal:?}").into())
        }
        Err(other) => Err(other.to_string().into()),
    }
}

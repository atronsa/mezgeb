//! Balance verification (step 1.13): every planted corruption is found,
//! with the exact account and entry; incremental runs check only what is
//! new; runs are recorded and checkpointed.
//!
//! The corruptions are planted as the superuser with
//! `session_replication_role = replica`, which switches off triggers and
//! foreign keys: exactly the kind of access that bypasses every other
//! protection, and that verification exists to catch.

mod support;

use std::time::Duration;

use mezgeb_core::{AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, Money};
use mezgeb_engine::{Finding, PgStore, Scope, VerificationReport, VerifyOptions, verify};
use sqlx::{Executor, PgConnection};
use support::{Server, TestDb, TestResult, exec, uuid7};
use uuid::Uuid;

const CHART: &str = r#"
name = "Verification chart"
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
"#;

const BANK: u64 = 1;
const ABEBE: u64 = 2;
const HANA: u64 = 3;
const SARA: u64 = 4;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000 + n).parse()
}

fn id(n: u64) -> Result<Uuid, uuid::Error> {
    Uuid::parse_str(&uuid7(n))
}

fn entry_uuid(n: u64) -> Result<Uuid, uuid::Error> {
    id(1_000 + n)
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn options(scope: Scope) -> VerifyOptions {
    VerifyOptions {
        scope,
        settle: Duration::ZERO,
        record: true,
    }
}

/// Bank, three wallets, and three entries:
/// 1. Dr bank 1,000 / Cr Abebe 1,000
/// 2. Dr Abebe 300 / Cr Hana 300
/// 3. Dr bank 500 / Cr Hana 500
async fn setup() -> Result<Option<(TestDb, PgStore)>, Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::new(db.app_pool().await?, client);
    let chart = Chart::from_toml(CHART)?;
    store.open_account(account(BANK)?, &chart, "1010").await?;
    store
        .open_account(account(ABEBE)?, &chart, "2010:cust-abebe")
        .await?;
    store
        .open_account(account(HANA)?, &chart, "2010:cust-hana")
        .await?;
    store
        .open_account(account(SARA)?, &chart, "2010:cust-sara")
        .await?;
    post(&mut store, 1, BANK, ABEBE, 1_000).await?;
    post(&mut store, 2, ABEBE, HANA, 300).await?;
    post(&mut store, 3, BANK, HANA, 500).await?;
    Ok(Some((db, store)))
}

async fn post(
    store: &mut PgStore,
    n: u64,
    debit: u64,
    credit: u64,
    amount: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    store
        .post(
            entry(n)?,
            EntryDraft::default()
                .debit(account(debit)?, etb(amount))
                .credit(account(credit)?, etb(amount)),
        )
        .await?;
    Ok(())
}

/// Runs SQL as the superuser with triggers and foreign keys switched off.
async fn tamper(db: &TestDb, sql: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = db.superuser().await?;
    exec(
        &mut conn,
        format!("SET session_replication_role = replica; {sql}"),
    )
    .await?;
    Ok(())
}

async fn run(db: &TestDb, scope: Scope) -> Result<VerificationReport, Box<dyn std::error::Error>> {
    let mut conn: PgConnection = db.verifier().await?;
    Ok(verify(&mut conn, &options(scope)).await?)
}

fn has(report: &VerificationReport, wanted: impl Fn(&Finding) -> bool) -> bool {
    report.findings.iter().any(wanted)
}

#[tokio::test]
async fn a_clean_ledger_verifies_clean() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    let report = run(&db, Scope::Full).await?;
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.lines_checked, 6);
    assert_eq!((report.from_sequence, report.to_sequence), (0, 3));
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Corruption 1: one line's amount changed.
#[tokio::test]
async fn a_tampered_line_amount_is_found() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    tamper(
        &db,
        &format!(
            "UPDATE journal_lines SET amount = 301 WHERE entry_id = '{}' AND line_no = 0",
            entry_uuid(2)?
        ),
    )
    .await?;
    let report = run(&db, Scope::Full).await?;
    eprintln!("{report}");
    let (e2, abebe) = (entry_uuid(2)?, id(ABEBE)?);
    assert!(has(&report, |f| matches!(f,
        Finding::EntryUnbalanced { entry, debits: 301, credits: 300, .. } if *entry == e2)));
    assert!(has(&report, |f| matches!(f,
        Finding::ChainBroken { account, entry, line_no: 0, .. } if *account == abebe && *entry == e2)));
    assert!(has(&report, |f| matches!(f,
        Finding::AccountTotals { account, stored: (300, 1_000), expected: (301, 1_000) }
            if *account == abebe)));
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

/// Corruption 2: one account's stored total changed.
#[tokio::test]
async fn a_tampered_account_total_is_found() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    tamper(
        &db,
        &format!(
            "UPDATE accounts SET credits_total = credits_total + 5 WHERE id = '{}'",
            id(HANA)?
        ),
    )
    .await?;
    let report = run(&db, Scope::Full).await?;
    eprintln!("{report}");
    let hana = id(HANA)?;
    assert!(has(&report, |f| matches!(f,
        Finding::AccountTotals { account, stored: (0, 805), expected: (0, 800) } if *account == hana)));
    assert!(has(&report, |f| matches!(
        f,
        Finding::GlobalUnbalanced {
            debits: 1_800,
            credits: 1_805,
            ..
        }
    )));
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

/// Corruption 3: one line deleted.
#[tokio::test]
async fn a_deleted_line_is_found() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    tamper(
        &db,
        &format!(
            "DELETE FROM journal_lines WHERE entry_id = '{}' AND line_no = 1",
            entry_uuid(3)?
        ),
    )
    .await?;
    let report = run(&db, Scope::Full).await?;
    eprintln!("{report}");
    let (e3, hana) = (entry_uuid(3)?, id(HANA)?);
    assert!(has(&report, |f| matches!(f,
        Finding::EntryTooFewLines { entry, lines: 1 } if *entry == e3)));
    assert!(has(&report, |f| matches!(f,
        Finding::EntryUnbalanced { entry, debits: 500, credits: 0, .. } if *entry == e3)));
    assert!(has(&report, |f| matches!(f,
        Finding::AccountTotals { account, stored: (0, 800), expected: (0, 300) } if *account == hana)));
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

/// Corruption 4: an entry deleted, its lines left behind.
#[tokio::test]
async fn a_deleted_entry_leaves_orphan_lines_that_are_found() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    tamper(
        &db,
        &format!(
            "DELETE FROM journal_entries WHERE id = '{}'",
            entry_uuid(2)?
        ),
    )
    .await?;
    let report = run(&db, Scope::Full).await?;
    eprintln!("{report}");
    let (e2, e3, hana) = (entry_uuid(2)?, entry_uuid(3)?, id(HANA)?);
    for line in [0, 1] {
        assert!(has(&report, |f| matches!(f,
            Finding::OrphanLine { entry, line_no } if *entry == e2 && *line_no == line)));
    }
    // Hana's later line no longer continues her chain.
    assert!(has(&report, |f| matches!(f,
        Finding::ChainBroken { account, entry, .. } if *account == hana && *entry == e3)));
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

/// Corruption 5: an entry inserted with no lines.
#[tokio::test]
async fn an_entry_without_lines_is_found() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    tamper(
        &db,
        &format!(
            "INSERT INTO journal_entries (id, client_id, idempotency_key, period_id)
             VALUES ('{}', '{}', 'planted', (SELECT id FROM periods WHERE status = 'open'))",
            entry_uuid(9)?,
            uuid7(0xc11e)
        ),
    )
    .await?;
    let report = run(&db, Scope::Full).await?;
    eprintln!("{report}");
    let e9 = entry_uuid(9)?;
    assert!(has(&report, |f| matches!(f,
        Finding::EntryTooFewLines { entry, lines: 0 } if *entry == e9)));
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

/// Incremental runs check only entries after the last clean checkpoint, and
/// still check every account's totals.
#[tokio::test]
async fn incremental_runs_check_what_is_new() -> TestResult {
    let Some((db, mut store)) = setup().await? else {
        return Ok(());
    };
    let first = run(&db, Scope::Incremental).await?;
    assert!(first.is_clean(), "{first}");
    assert_eq!(
        (first.from_sequence, first.to_sequence, first.lines_checked),
        (0, 3, 6)
    );

    post(&mut store, 4, BANK, SARA, 70).await?;
    let second = run(&db, Scope::Incremental).await?;
    assert!(second.is_clean(), "{second}");
    assert_eq!(
        (
            second.from_sequence,
            second.to_sequence,
            second.lines_checked
        ),
        (3, 4, 2)
    );

    // A tampered new line is found.
    post(&mut store, 5, SARA, HANA, 20).await?;
    tamper(
        &db,
        &format!(
            "UPDATE journal_lines SET amount = 21 WHERE entry_id = '{}' AND line_no = 1",
            entry_uuid(5)?
        ),
    )
    .await?;
    let dirty = run(&db, Scope::Incremental).await?;
    eprintln!("{dirty}");
    let e5 = entry_uuid(5)?;
    assert!(has(&dirty, |f| matches!(f,
        Finding::EntryUnbalanced { entry, .. } if *entry == e5)));
    // A dirty run does not move the checkpoint: the next run starts at the
    // same place and finds the same problem.
    let again = run(&db, Scope::Incremental).await?;
    assert_eq!(again.from_sequence, 4);
    assert!(!again.is_clean());

    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

/// An incremental run checks every account total, including accounts with
/// nothing new; but an old line, already verified, is only re-checked by a
/// full run. (The hash chain of step 2.09 makes old-row tampering cheap to
/// detect.)
#[tokio::test]
async fn what_incremental_runs_do_and_do_not_recheck() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    assert!(run(&db, Scope::Incremental).await?.is_clean());

    // An untouched account's total: found by an incremental run.
    tamper(
        &db,
        &format!(
            "UPDATE accounts SET debits_total = debits_total + 1 WHERE id = '{}'",
            id(BANK)?
        ),
    )
    .await?;
    let report = run(&db, Scope::Incremental).await?;
    let bank = id(BANK)?;
    assert!(has(&report, |f| matches!(f,
        Finding::AccountTotals { account, .. } if *account == bank)));
    tamper(
        &db,
        &format!(
            "UPDATE accounts SET debits_total = debits_total - 1 WHERE id = '{}'",
            id(BANK)?
        ),
    )
    .await?;

    // An old, already verified line: missed by an incremental run, found
    // by a full one.
    tamper(
        &db,
        &format!(
            "UPDATE journal_lines SET amount = 999 WHERE entry_id = '{}' AND line_no = 0",
            entry_uuid(1)?
        ),
    )
    .await?;
    assert!(run(&db, Scope::Incremental).await?.is_clean());
    let full = run(&db, Scope::Full).await?;
    assert!(!full.is_clean());
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

/// Entries newer than the settle time are left for the next run.
#[tokio::test]
async fn recent_entries_wait_for_the_next_run() -> TestResult {
    let Some((db, mut store)) = setup().await? else {
        return Ok(());
    };
    assert!(run(&db, Scope::Incremental).await?.is_clean());
    post(&mut store, 4, BANK, SARA, 70).await?;
    let mut conn = db.verifier().await?;
    let settled = VerifyOptions {
        scope: Scope::Incremental,
        settle: Duration::from_secs(60),
        record: true,
    };
    let report = verify(&mut conn, &settled).await?;
    assert!(report.is_clean(), "{report}");
    assert_eq!((report.from_sequence, report.to_sequence), (3, 3));
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Runs are recorded and append-only; the verifier can post nothing.
#[tokio::test]
async fn runs_are_recorded_and_the_verifier_is_read_only() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    let report = run(&db, Scope::Full).await?;
    let mut owner = db.owner().await?;
    let (result, lines): (String, i64) =
        sqlx::query_as("SELECT result, lines_checked FROM verification_runs WHERE id = $1")
            .bind(report.run_id)
            .fetch_one(&mut owner)
            .await?;
    assert_eq!((result.as_str(), lines), ("clean", 6));
    let checkpoints: i64 =
        sqlx::query_scalar("SELECT count(*) FROM verification_checkpoints WHERE run_id = $1")
            .bind(report.run_id)
            .fetch_one(&mut owner)
            .await?;
    assert_eq!(checkpoints, 3, "bank, Abebe and Hana have lines");

    let state = |r: Result<_, sqlx::Error>| {
        r.err().and_then(|e| {
            e.as_database_error()
                .and_then(|d| d.code())
                .map(|c| c.into_owned())
        })
    };
    assert_eq!(
        state(
            owner
                .execute("UPDATE verification_runs SET result = 'dirty'")
                .await
        )
        .as_deref(),
        Some("MZ001")
    );
    let mut verifier = db.verifier().await?;
    assert_eq!(
        state(
            verifier
                .execute("UPDATE accounts SET credits_total = 0")
                .await
        )
        .as_deref(),
        Some("42501")
    );
    assert_eq!(
        state(verifier.execute("DELETE FROM verification_runs").await).as_deref(),
        Some("42501")
    );
    drop(owner);
    drop(verifier);
    drop(store);
    db.drop().await?;
    Ok(())
}

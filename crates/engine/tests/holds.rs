//! Holds on PostgreSQL (step 1.14): the engine's operations, their
//! idempotency, the database's own guards, and verification of INV-18.

mod support;

use std::time::Duration;

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, HoldId, HoldStatus, Holds,
    IdempotencyKey, Ledger, LedgerError, Money, PositiveAmount, PostingError, Timestamp,
};
use mezgeb_engine::{
    CaptureHold, Finding, HoldRequest, PgStore, PlaceHold, PostOptions, Scope, VerifyOptions,
    capture_hold, place_hold, verify,
};
use sqlx::Executor;
use support::{Server, TestDb, TestResult, exec, uuid7};

const CHART: &str = r#"
name = "Holds chart"
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
code = 2020
name = "Pending withdrawals"
type = "liability"
kind = "control"
prefix = "wd"
status = "active"

[[accounts]]
code = 2030
name = "Merchant balances"
type = "liability"
kind = "control"
prefix = "merch"
status = "active"
"#;

const BANK: u64 = 1;
const ABEBE: u64 = 2;
const COFFEE: u64 = 3;
const WD1: u64 = 4;
const WD2: u64 = 5;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000 + n).parse()
}

fn hold(n: u64) -> Result<HoldId, mezgeb_core::IdError> {
    uuid7(5_000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn positive(amount: i64) -> Result<PositiveAmount, mezgeb_core::MoneyError> {
    PositiveAmount::new(etb(amount))
}

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros)
}

/// Bank, Abebe's wallet funded with 1,000, Merkato Coffee, and two empty
/// pending-withdrawal accounts to hold money in (as in E02).
async fn setup() -> Result<Option<(TestDb, PgStore)>, Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::new(db.app_pool().await?, client);
    let chart = Chart::from_toml(CHART)?;
    for (n, reference) in [
        (BANK, "1010"),
        (ABEBE, "2010:cust-abebe"),
        (COFFEE, "2030:merch-coffee"),
        (WD1, "2020:wd-0001"),
        (WD2, "2020:wd-0002"),
    ] {
        store.open_account(account(n)?, &chart, reference).await?;
    }
    store
        .post(
            entry(0)?,
            EntryDraft::default()
                .debit(account(BANK)?, etb(1_000))
                .credit(account(ABEBE)?, etb(1_000)),
        )
        .await?;
    Ok(Some((db, store)))
}

async fn balance(store: &mut PgStore, n: u64) -> Result<i64, Box<dyn std::error::Error>> {
    let account = store.get_account(account(n)?).await?.ok_or("no account")?;
    Ok(account.balance().amount())
}

/// E02 and E03 as a hold: request a withdrawal of 300 (place), the bank
/// sends it (capture into the bank account).
#[tokio::test]
async fn a_withdrawal_is_a_hold_that_is_captured() -> TestResult {
    let Some((db, mut store)) = setup().await? else {
        return Ok(());
    };
    let placed = store
        .place_hold(
            hold(1)?,
            entry(1)?,
            account(ABEBE)?,
            account(WD1)?,
            positive(300)?,
            at(1_000),
        )
        .await?;
    assert_eq!(placed.status(), HoldStatus::Active);
    assert_eq!(balance(&mut store, ABEBE).await?, 700);
    assert_eq!(balance(&mut store, WD1).await?, 300);

    // Only 700 is left to spend.
    let spend = EntryDraft::default()
        .debit(account(ABEBE)?, etb(800))
        .credit(account(COFFEE)?, etb(800));
    assert_eq!(
        store.post(entry(2)?, spend).await,
        Err(LedgerError::Posting(PostingError::InsufficientFunds {
            line: 0
        }))
    );

    let sent = store
        .capture_hold(hold(1)?, entry(3)?, account(BANK)?, positive(300)?, at(10))
        .await?;
    assert_eq!(
        (sent.status(), sent.captured()),
        (HoldStatus::Captured, 300)
    );
    assert_eq!(balance(&mut store, BANK).await?, 700);
    assert_eq!(balance(&mut store, WD1).await?, 0);
    assert_eq!(
        store
            .capture_hold(hold(1)?, entry(4)?, account(BANK)?, positive(1)?, at(10))
            .await,
        Err(LedgerError::HoldNotActive {
            hold: hold(1)?,
            status: HoldStatus::Captured
        })
    );
    drop(store);
    db.drop().await?;
    Ok(())
}

/// A partial capture and the release of the rest add up to the original.
#[tokio::test]
async fn a_partial_capture_returns_the_rest() -> TestResult {
    let Some((db, mut store)) = setup().await? else {
        return Ok(());
    };
    store
        .place_hold(
            hold(1)?,
            entry(1)?,
            account(ABEBE)?,
            account(WD1)?,
            positive(600)?,
            at(1_000),
        )
        .await?;
    let captured = store
        .capture_hold(
            hold(1)?,
            entry(2)?,
            account(COFFEE)?,
            positive(250)?,
            at(10),
        )
        .await?;
    assert_eq!(captured.captured(), 250);
    assert_eq!(balance(&mut store, COFFEE).await?, 250);
    assert_eq!(balance(&mut store, ABEBE).await?, 750);
    assert_eq!(balance(&mut store, WD1).await?, 0);

    // Release and expiry return everything.
    store
        .place_hold(
            hold(2)?,
            entry(3)?,
            account(ABEBE)?,
            account(WD2)?,
            positive(100)?,
            at(50),
        )
        .await?;
    assert_eq!(
        store.expire_hold(hold(2)?, entry(4)?, at(49)).await,
        Err(LedgerError::HoldNotExpired(hold(2)?))
    );
    let expired = store.expire_hold(hold(2)?, entry(4)?, at(50)).await?;
    assert_eq!(expired.status(), HoldStatus::Expired);
    assert_eq!(balance(&mut store, ABEBE).await?, 750);
    assert_eq!(store.all_holds().await?.len(), 2);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Hold operations are idempotent like any posting.
#[tokio::test]
async fn hold_operations_are_idempotent() -> TestResult {
    let Some((db, store)) = setup().await? else {
        return Ok(());
    };
    let mut conn = db.app().await?;
    let options = PostOptions::default();
    let request = |n: u64, key: &str| -> Result<HoldRequest, Box<dyn std::error::Error>> {
        Ok(HoldRequest {
            entry_id: entry(n)?,
            client_id: uuid7(0xc11e).parse()?,
            idempotency_key: IdempotencyKey::new(key)?,
        })
    };
    let place = PlaceHold {
        hold: hold(1)?,
        account: account(ABEBE)?,
        held: account(WD1)?,
        amount: positive(300)?,
        expires_at: at(1_000),
    };
    let first = place_hold(&mut conn, &request(1, "hold-1")?, place, &options).await?;
    // A retry, with a new entry ID as a fresh attempt would have.
    let again = place_hold(&mut conn, &request(2, "hold-1")?, place, &options).await?;
    assert_eq!(again, first);
    // The same key for a different hold request.
    let bigger = PlaceHold {
        amount: positive(301)?,
        ..place
    };
    assert_eq!(
        place_hold(&mut conn, &request(3, "hold-1")?, bigger, &options).await,
        Err(LedgerError::IdempotencyKeyReused)
    );

    let capture = CaptureHold {
        hold: hold(1)?,
        destination: account(BANK)?,
        amount: positive(300)?,
        now: at(10),
    };
    let captured = capture_hold(&mut conn, &request(4, "send-1")?, capture, &options).await?;
    let repeat = capture_hold(&mut conn, &request(5, "send-1")?, capture, &options).await?;
    assert_eq!(repeat, captured);
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Nothing but its hold posts to a held account.
#[tokio::test]
async fn held_accounts_are_protected_on_postgresql() -> TestResult {
    let Some((db, mut store)) = setup().await? else {
        return Ok(());
    };
    store
        .place_hold(
            hold(1)?,
            entry(1)?,
            account(ABEBE)?,
            account(WD1)?,
            positive(300)?,
            at(1_000),
        )
        .await?;
    let raid = EntryDraft::default()
        .debit(account(WD1)?, etb(300))
        .credit(account(COFFEE)?, etb(300));
    assert_eq!(
        store.post(entry(2)?, raid).await,
        Err(LedgerError::AccountIsHeld(account(WD1)?))
    );
    assert_eq!(
        store
            .place_hold(
                hold(2)?,
                entry(2)?,
                account(ABEBE)?,
                account(WD1)?,
                positive(1)?,
                at(9)
            )
            .await,
        Err(LedgerError::InvalidHeldAccount(account(WD1)?))
    );
    drop(store);
    db.drop().await?;
    Ok(())
}

/// The database enforces the hold rules even when the engine is bypassed.
#[tokio::test]
async fn the_database_guards_holds() -> TestResult {
    let Some((db, mut store)) = setup().await? else {
        return Ok(());
    };
    store
        .place_hold(
            hold(1)?,
            entry(1)?,
            account(ABEBE)?,
            account(WD1)?,
            positive(300)?,
            at(1_000),
        )
        .await?;
    store.release_hold(hold(1)?, entry(2)?).await?;
    store
        .place_hold(
            hold(2)?,
            entry(3)?,
            account(ABEBE)?,
            account(WD2)?,
            positive(100)?,
            at(1_000),
        )
        .await?;

    let state = |r: Result<_, sqlx::Error>| {
        r.err().and_then(|e| {
            e.as_database_error()
                .and_then(|d| d.code())
                .map(|c| c.into_owned())
        })
    };
    let (ended, active) = (uuid7(5_001), uuid7(5_002));
    let mut owner = db.owner().await?;
    for (sql, code) in [
        // An ended hold never changes again (INV-16).
        (
            format!(
                "UPDATE holds SET status = 'active', close_entry_id = NULL WHERE id = '{ended}'"
            ),
            "MZ006",
        ),
        // An active hold's amount is fixed.
        (
            format!("UPDATE holds SET amount = 999 WHERE id = '{active}'"),
            "MZ006",
        ),
        // A capture never exceeds the hold (INV-17).
        (
            format!(
                "UPDATE holds SET status = 'captured', captured = 101, close_entry_id = '{}' WHERE id = '{active}'",
                uuid7(1_002)
            ),
            "23514",
        ),
        // Holds are never deleted.
        (format!("DELETE FROM holds WHERE id = '{active}'"), "MZ001"),
    ] {
        assert_eq!(
            state(owner.execute(sqlx::AssertSqlSafe(sql.clone())).await).as_deref(),
            Some(code),
            "{sql}"
        );
    }
    let mut app = db.app().await?;
    assert_eq!(
        state(
            app.execute(sqlx::AssertSqlSafe(format!(
                "UPDATE holds SET amount = 1 WHERE id = '{active}'"
            )))
            .await
        )
        .as_deref(),
        Some("42501")
    );
    drop(owner);
    drop(app);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Verification finds a held account that does not hold its hold's amount.
#[tokio::test]
async fn verification_checks_held_balances() -> TestResult {
    let Some((db, mut store)) = setup().await? else {
        return Ok(());
    };
    store
        .place_hold(
            hold(1)?,
            entry(1)?,
            account(ABEBE)?,
            account(WD1)?,
            positive(300)?,
            at(1_000),
        )
        .await?;
    let options = VerifyOptions {
        scope: Scope::Full,
        settle: Duration::ZERO,
        record: false,
    };
    assert!(
        verify(&mut db.verifier().await?, &options)
            .await?
            .is_clean()
    );

    // As the superuser: the held account's total no longer matches.
    let mut superuser = db.superuser().await?;
    exec(
        &mut superuser,
        format!(
            "SET session_replication_role = replica;
             UPDATE accounts SET credits_total = credits_total - 1 WHERE id = '{}'",
            uuid7(WD1)
        ),
    )
    .await?;
    let report = verify(&mut db.verifier().await?, &options).await?;
    eprintln!("{report}");
    let (held, hold_uuid) = (
        uuid::Uuid::parse_str(&uuid7(WD1))?,
        uuid::Uuid::parse_str(&uuid7(5_001))?,
    );
    assert!(report.findings.iter().any(|f| matches!(f,
        Finding::HoldBalance { hold, account, stored: 299, expected: 300 }
            if *hold == hold_uuid && *account == held)));
    drop(superuser);
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

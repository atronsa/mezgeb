//! Failure injection (step 1.15): every kind of write is made to fail at
//! each of the four fault points, then retried with the same idempotency
//! key. Whatever the point, the retry leaves exactly one entry for the key,
//! and the ledger verifies clean.
//!
//! A failure before `COMMIT` is a crash: nothing is written, and the retry
//! does the work. A failure after it is a lost response: everything is
//! written, and the retry returns what was done instead of doing it again.

mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mezgeb_core::{
    AccountId, ActorId, Authorization, Chart, ClientId, Currency, EntryDraft, EntryId, HoldId,
    HoldStatus, IdempotencyKey, Ledger, LedgerError, Money, PositiveAmount, ReasonCode, Timestamp,
};
use mezgeb_engine::{
    CaptureHold, FaultPoint, HoldRequest, PgStore, PlaceHold, PostOptions, PostRequest, Scope,
    VerifyOptions, capture_hold, expire_due_holds, expire_hold, place_hold, post_entry_with,
    release_hold, reverse_entry_with, verify,
};
use sqlx::PgPool;
use support::{Server, TestDb, TestResult, uuid7};

const CHART: &str = r#"
name = "Fault chart"
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
"#;

const BANK: u64 = 1;
const ABEBE: u64 = 2;
const HANA: u64 = 3;
/// Held accounts of holds 1 to 4.
const WD: u64 = 10;

type Fail = Box<dyn std::error::Error>;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn hold(n: u64) -> Result<HoldId, mezgeb_core::IdError> {
    uuid7(5_000 + n).parse()
}

fn client() -> Result<ClientId, mezgeb_core::IdError> {
    uuid7(0xc11e).parse()
}

/// A fresh entry ID for every attempt, as the ledger would assign: a retry
/// is a new attempt at the same request, identified by its key.
fn fresh_entry_id() -> Result<EntryId, mezgeb_core::IdError> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    uuid7(1_000_000 + NEXT.fetch_add(1, Ordering::SeqCst)).parse()
}

fn key(name: &str) -> Result<IdempotencyKey, mezgeb_core::IdempotencyKeyError> {
    IdempotencyKey::new(name)
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

/// The bank, Abebe funded with 10,000, Hana, and four empty held accounts.
async fn setup() -> Result<Option<(TestDb, PgPool)>, Fail> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    let pool = db.app_pool_of(4).await?;
    let mut store = PgStore::new(pool.clone(), client()?);
    let chart = Chart::from_toml(CHART)?;
    store.open_account(account(BANK)?, &chart, "1010").await?;
    store
        .open_account(account(ABEBE)?, &chart, "2010:cust-abebe")
        .await?;
    store
        .open_account(account(HANA)?, &chart, "2010:cust-hana")
        .await?;
    for n in 1..=4 {
        store
            .open_account(account(WD + n)?, &chart, &format!("2020:wd-{n}"))
            .await?;
    }
    store
        .post(
            fresh_entry_id()?,
            EntryDraft::default()
                .debit(account(BANK)?, etb(10_000))
                .credit(account(ABEBE)?, etb(10_000)),
        )
        .await?;
    Ok(Some((db, pool)))
}

async fn entry_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM journal_entries")
        .fetch_one(pool)
        .await
}

async fn entries_with_key(pool: &PgPool, key: &str) -> Result<i64, Fail> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM journal_entries WHERE client_id = $1 AND idempotency_key = $2",
    )
    .bind(client()?.as_uuid())
    .bind(key)
    .fetch_one(pool)
    .await?)
}

async fn balance(pool: &PgPool, n: u64) -> Result<i64, Fail> {
    let mut store = PgStore::new(pool.clone(), client()?);
    let account = store.get_account(account(n)?).await?.ok_or("no account")?;
    Ok(account.balance().amount())
}

/// Runs `attempt` once failing at `point`, then again without a fault, and
/// checks what each left behind. `key` is the attempt's idempotency key.
async fn fail_then_retry<T>(
    db: &TestDb,
    pool: &PgPool,
    point: FaultPoint,
    key: &str,
    attempt: impl AsyncFn(PostOptions) -> Result<T, LedgerError>,
) -> Result<T, Fail> {
    let before = entry_count(pool).await?;
    let faulty = PostOptions {
        fault: Some(point),
        ..PostOptions::default()
    };
    match attempt(faulty).await {
        Err(LedgerError::Storage(message)) if message.starts_with("test hook") => {}
        Err(other) => return Err(format!("{key} at {point:?}: {other}").into()),
        Ok(_) => return Err(format!("{key} at {point:?}: the fault did not fire").into()),
    }
    // A crash wrote nothing; a lost response wrote everything.
    let written = i64::from(point == FaultPoint::AfterCommit);
    assert_eq!(
        entry_count(pool).await? - before,
        written,
        "{key} at {point:?}: after the fault"
    );

    let retried = attempt(PostOptions::default())
        .await
        .map_err(|e| format!("{key} retried after {point:?}: {e}"))?;
    assert_eq!(
        entry_count(pool).await? - before,
        1,
        "{key} at {point:?}: after the retry"
    );
    assert_eq!(
        entries_with_key(pool, key).await?,
        1,
        "{key} at {point:?}: entries with its key"
    );
    let options = VerifyOptions {
        scope: Scope::Full,
        settle: Duration::ZERO,
        record: false,
    };
    let report = verify(&mut db.verifier().await?, &options).await?;
    assert!(report.is_clean(), "{key} at {point:?}:\n{report}");
    Ok(retried)
}

/// Every kind of write, each failing at `point` once and then retried.
async fn every_write_survives(point: FaultPoint) -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let client = client()?;
    let request = |name: &str, draft: EntryDraft| -> Result<PostRequest, Fail> {
        Ok(PostRequest::new(
            fresh_entry_id()?,
            client,
            key(name)?,
            draft,
        ))
    };
    let hold_request = |name: &str| -> Result<HoldRequest, Fail> {
        Ok(HoldRequest {
            entry_id: fresh_entry_id()?,
            client_id: client,
            idempotency_key: key(name)?,
        })
    };
    let payment = EntryDraft::default()
        .debit(account(ABEBE)?, etb(500))
        .credit(account(HANA)?, etb(500));
    let compensation = EntryDraft::default()
        .debit(account(ABEBE)?, etb(100))
        .credit(account(HANA)?, etb(100));
    let approved = Authorization {
        reason: ReasonCode::new("compensation")?,
        requested_by: ActorId::new("ops:1")?,
        approved_by: Some(ActorId::new("ops:2")?),
    };
    let booking_error = Authorization {
        reason: ReasonCode::new("booking_error")?,
        requested_by: ActorId::new("ops:1")?,
        approved_by: None,
    };
    let fault_error = |e: Fail| LedgerError::Storage(format!("test setup: {e}"));

    // A payment, an adjustment, and the payment's reversal.
    let paid = fail_then_retry(&db, &pool, point, "pay", async |options| {
        let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
        let request = request("pay", payment.clone()).map_err(fault_error)?;
        post_entry_with(&mut conn, request, &options).await
    })
    .await?;
    fail_then_retry(&db, &pool, point, "adjust", async |options| {
        let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
        let request = request("adjust", compensation.clone())
            .map_err(fault_error)?
            .authorized(approved.clone());
        post_entry_with(&mut conn, request, &options).await
    })
    .await?;
    fail_then_retry(&db, &pool, point, "reverse", async |options| {
        let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
        let request = request("reverse", EntryDraft::default())
            .map_err(fault_error)?
            .authorized(booking_error.clone());
        reverse_entry_with(&mut conn, paid.id(), request, &options).await
    })
    .await?;

    // Four holds: captured in part, released, expired, and expired by the
    // worker.
    for (n, amount, expires) in [(1, 400, 1_000), (2, 50, 1_000), (3, 60, 100), (4, 70, 100)] {
        let name = format!("place-{n}");
        let place = PlaceHold {
            hold: hold(n)?,
            account: account(ABEBE)?,
            held: account(WD + n)?,
            amount: positive(amount)?,
            expires_at: at(expires),
        };
        fail_then_retry(&db, &pool, point, &name, async |options| {
            let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
            let request = hold_request(&name).map_err(fault_error)?;
            place_hold(&mut conn, &request, place, &options).await
        })
        .await?;
    }
    let captured = fail_then_retry(&db, &pool, point, "capture-1", async |options| {
        let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
        let request = hold_request("capture-1").map_err(fault_error)?;
        let capture = CaptureHold {
            hold: hold(1).map_err(|e| fault_error(e.into()))?,
            destination: account(BANK).map_err(|e| fault_error(e.into()))?,
            amount: positive(300).map_err(|e| fault_error(e.into()))?,
            now: at(10),
        };
        capture_hold(&mut conn, &request, capture, &options).await
    })
    .await?;
    assert_eq!(
        (captured.status(), captured.captured()),
        (HoldStatus::Captured, 300)
    );
    let released = fail_then_retry(&db, &pool, point, "release-2", async |options| {
        let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
        let request = hold_request("release-2").map_err(fault_error)?;
        let id = hold(2).map_err(|e| fault_error(e.into()))?;
        release_hold(&mut conn, &request, id, &options).await
    })
    .await?;
    assert_eq!(released.status(), HoldStatus::Released);
    let expired = fail_then_retry(&db, &pool, point, "expire-3", async |options| {
        let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
        let request = hold_request("expire-3").map_err(fault_error)?;
        let id = hold(3).map_err(|e| fault_error(e.into()))?;
        expire_hold(&mut conn, &request, id, at(200), &options).await
    })
    .await?;
    assert_eq!(expired.status(), HoldStatus::Expired);
    let worker_key = format!("hold-expire:{}", hold(4)?);
    let by_worker = fail_then_retry(&db, &pool, point, &worker_key, async |options| {
        let mut conn = pool.acquire().await.map_err(|e| fault_error(e.into()))?;
        expire_due_holds(&mut conn, client, at(200), 10, &options).await
    })
    .await?;
    // After a lost response the worker's retry finds nothing left to do; after
    // a crash it expires the hold itself.
    let expected: Vec<HoldId> = if point == FaultPoint::AfterCommit {
        vec![]
    } else {
        vec![hold(4)?]
    };
    assert_eq!(
        by_worker.iter().map(|h| h.id()).collect::<Vec<_>>(),
        expected
    );

    // The money moved exactly once per request: 500 paid and reversed, 100
    // compensated, 300 of a 400 hold captured to the bank, the rest of every
    // hold returned.
    assert_eq!(balance(&pool, ABEBE).await?, 10_000 - 100 - 300);
    assert_eq!(balance(&pool, HANA).await?, 100);
    assert_eq!(balance(&pool, BANK).await?, 10_000 - 300);
    for n in 1..=4 {
        assert_eq!(balance(&pool, WD + n).await?, 0);
    }
    eprintln!("{point:?}: 11 writes failed once and were retried; one entry each, verify clean");
    pool.close().await;
    db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn a_crash_before_locking_is_retried_once() -> TestResult {
    every_write_survives(FaultPoint::BeforeLock).await
}

#[tokio::test]
async fn a_crash_after_writing_the_lines_is_retried_once() -> TestResult {
    every_write_survives(FaultPoint::AfterLines).await
}

#[tokio::test]
async fn a_crash_before_commit_is_retried_once() -> TestResult {
    every_write_survives(FaultPoint::BeforeCommit).await
}

#[tokio::test]
async fn a_lost_response_after_commit_is_retried_once() -> TestResult {
    every_write_survives(FaultPoint::AfterCommit).await
}

/// The four points cover every write in order, so a test that loops over
/// them misses none.
#[test]
fn fault_points_are_in_order() {
    assert_eq!(
        FaultPoint::ALL,
        [
            FaultPoint::BeforeLock,
            FaultPoint::AfterLines,
            FaultPoint::BeforeCommit,
            FaultPoint::AfterCommit,
        ]
    );
}

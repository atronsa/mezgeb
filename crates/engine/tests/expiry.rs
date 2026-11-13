//! The hold expiry worker (step 1.14): it expires only due holds, a capture
//! racing it has exactly one winner, a worker killed mid-batch can be
//! restarted without expiring any hold twice, and the held balance equals
//! the sum of active holds at every moment under concurrency (INV-18).

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, HoldId, HoldStatus, Holds,
    IdempotencyKey, Ledger, LedgerError, Money, PositiveAmount, Timestamp,
};
use mezgeb_engine::{HoldRequest, PgStore, PostOptions, expire_due_holds, expire_hold};
use sqlx::{Connection, PgPool};
use support::{Server, TestDb, TestResult, uuid7};

/// An error that can cross from a spawned task.
type Fail = Box<dyn std::error::Error + Send + Sync>;

const CHART: &str = r#"
name = "Expiry chart"
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

/// The concurrency tests in this file run one at a time: together they
/// would compete for the same test server and measure its load instead.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const POOL: u32 = 12;
const BANK: u64 = 1;

/// Wallet `n`, its accounts from 100 on.
fn wallet(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(100 + n).parse()
}

/// The held account of hold `n`, from 100,000 on.
fn held(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(100_000 + n).parse()
}

fn hold(n: u64) -> Result<HoldId, mezgeb_core::IdError> {
    uuid7(200_000 + n).parse()
}

/// Entry IDs, from 300,000 on.
fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(300_000 + n).parse()
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

fn client() -> Result<ClientId, mezgeb_core::IdError> {
    uuid7(0xc11e).parse()
}

/// A migrated database, a store on a pool of `POOL` connections, the bank
/// and `wallets` wallets funded with `funds` each.
async fn setup(
    wallets: u64,
    funds: i64,
) -> Result<Option<(TestDb, PgStore, Chart)>, Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    // As in the other concurrency tests: correctness, not latency, so a
    // task may wait longer for a hot row on a busy test server.
    let options = PostOptions {
        lock_timeout: Duration::from_secs(20),
        ..PostOptions::default()
    };
    let mut store = PgStore::with_options(db.app_pool_of(POOL).await?, client()?, options);
    let chart = Chart::from_toml(CHART)?;
    store
        .open_account(uuid7(BANK).parse()?, &chart, "1010")
        .await?;
    for n in 0..wallets {
        store
            .open_account(wallet(n)?, &chart, &format!("2010:cust-{n}"))
            .await?;
        store
            .post(
                uuid7(400_000 + n).parse()?,
                EntryDraft::default()
                    .debit(uuid7(BANK).parse()?, etb(funds))
                    .credit(wallet(n)?, etb(funds)),
            )
            .await?;
    }
    Ok(Some((db, store, chart)))
}

/// Opens hold `n`'s held account and places the hold on wallet `on`.
async fn place(
    store: &mut PgStore,
    chart: &Chart,
    n: u64,
    on: u64,
    amount: i64,
    expires_at: i64,
) -> Result<(), String> {
    let placed = async {
        store
            .open_account(held(n)?, chart, &format!("2020:wd-{n}"))
            .await?;
        store
            .place_hold(
                hold(n)?,
                entry(n)?,
                wallet(on)?,
                held(n)?,
                positive(amount)?,
                at(expires_at),
            )
            .await?;
        Ok::<_, Fail>(())
    };
    placed.await.map_err(|e| format!("placing hold {n}: {e}"))
}

async fn balance(store: &mut PgStore, account: AccountId) -> Result<i64, LedgerError> {
    let account = store
        .get_account(account)
        .await?
        .ok_or(LedgerError::AccountNotFound(account))?;
    Ok(account.balance().amount())
}

async fn status_of(store: &mut PgStore, n: u64) -> Result<HoldStatus, Box<dyn std::error::Error>> {
    let id = hold(n)?;
    let holds = store.all_holds().await?;
    let found = holds.iter().find(|h| h.id() == id).ok_or("no such hold")?;
    Ok(found.status())
}

/// How many entries ended each hold, for holds ended more than once.
async fn ended_twice(pool: &PgPool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT metadata->>'hold', count(*)
         FROM journal_entries
         WHERE metadata->>'hold_action' IN ('capture', 'release', 'expire')
         GROUP BY 1
         HAVING count(*) > 1",
    )
    .fetch_all(pool)
    .await
}

/// INV-18 in one statement, so in one snapshot: the balance of every held
/// account, and the amount of every active hold.
async fn held_and_active<'e>(on: impl sqlx::PgExecutor<'e>) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT
             COALESCE(SUM(CASE WHEN a.account_type IN ('asset', 'expense')
                               THEN a.debits_total - a.credits_total
                               ELSE a.credits_total - a.debits_total END), 0)::bigint,
             COALESCE(SUM(h.amount) FILTER (WHERE h.status = 'active'), 0)::bigint
         FROM holds h
         JOIN accounts a ON a.id = h.held_account_id",
    )
    .fetch_one(on)
    .await
}

/// A small deterministic random sequence, so a failure can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

/// The worker expires due holds only, oldest expiry first, at most `limit`
/// at a time, and leaves ended holds alone. Its expiries are idempotent
/// under the key `hold-expire:<hold id>`.
#[tokio::test]
async fn the_worker_expires_only_due_holds() -> TestResult {
    let Some((db, mut store, chart)) = setup(1, 1_000).await? else {
        return Ok(());
    };
    place(&mut store, &chart, 1, 0, 100, 300).await?;
    place(&mut store, &chart, 2, 0, 200, 100).await?;
    place(&mut store, &chart, 3, 0, 300, 200).await?;
    place(&mut store, &chart, 4, 0, 50, 100).await?;
    store.release_hold(hold(4)?, entry(1_004)?).await?;
    assert_eq!(balance(&mut store, wallet(0)?).await?, 400);

    // Nothing is due yet.
    assert!(store.expire_due_holds(at(99), 10).await?.is_empty());

    // Due at 200: holds 2 and 3, one at a time with a limit of 1.
    let first = store.expire_due_holds(at(200), 1).await?;
    let ids: Vec<HoldId> = first.iter().map(|h| h.id()).collect();
    assert_eq!(ids, [hold(2)?]);
    let rest = store.expire_due_holds(at(200), 10).await?;
    let ids: Vec<HoldId> = rest.iter().map(|h| h.id()).collect();
    assert_eq!(ids, [hold(3)?]);
    assert!(rest.iter().all(|h| h.status() == HoldStatus::Expired));
    assert!(store.expire_due_holds(at(200), 10).await?.is_empty());

    assert_eq!(status_of(&mut store, 1).await?, HoldStatus::Active);
    assert_eq!(status_of(&mut store, 4).await?, HoldStatus::Released);
    assert_eq!(balance(&mut store, wallet(0)?).await?, 900);
    assert_eq!(balance(&mut store, held(2)?).await?, 0);
    assert_eq!(balance(&mut store, held(3)?).await?, 0);

    // Asking again under the worker's key returns the same expiry.
    let mut conn = store.pool().acquire().await?;
    let again = expire_hold(
        &mut conn,
        &HoldRequest {
            entry_id: entry(9_999)?,
            client_id: client()?,
            idempotency_key: IdempotencyKey::new(format!("hold-expire:{}", hold(2)?))?,
        },
        hold(2)?,
        at(500),
        &PostOptions::default(),
    )
    .await?;
    assert_eq!(again, first[0]);
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Exit gate: a capture and the worker race for the same due-at-the-edge
/// holds, 50 rounds in a row. Each hold ends exactly once: captured if the
/// capture won, expired if the worker did, and never both.
#[tokio::test]
async fn capture_racing_expiry_has_exactly_one_winner() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    const ROUNDS: u64 = 50;
    const HOLDS: u64 = 8;
    const AMOUNT: i64 = 10;
    const EXPIRES: i64 = 1_000;
    let Some((db, mut store, chart)) = setup(1, 100_000).await? else {
        return Ok(());
    };
    let mut captured_total = 0;
    let (mut captures_won, mut expiries_won) = (0, 0);
    for round in 0..ROUNDS {
        let first = round * HOLDS;
        for n in first..first + HOLDS {
            place(&mut store, &chart, n, 0, AMOUNT, EXPIRES).await?;
        }
        let start = Arc::new(tokio::sync::Barrier::new(HOLDS as usize + 2));
        let mut captures = Vec::new();
        for n in first..first + HOLDS {
            let mut store = store.clone();
            let start = start.clone();
            captures.push(tokio::spawn(async move {
                start.wait().await;
                let capture = async {
                    // One microsecond before expiry the capture is still allowed.
                    let result = store
                        .capture_hold(
                            hold(n)?,
                            entry(10_000 + n)?,
                            uuid7(BANK).parse()?,
                            positive(AMOUNT)?,
                            at(EXPIRES - 1),
                        )
                        .await;
                    Ok::<_, Fail>(result)
                };
                capture.await.map_err(|e| e.to_string())
            }));
        }
        let mut workers = Vec::new();
        for _ in 0..2 {
            let store = store.clone();
            let start = start.clone();
            workers.push(tokio::spawn(async move {
                start.wait().await;
                store.expire_due_holds(at(EXPIRES), usize::MAX).await
            }));
        }
        let mut expired = Vec::new();
        for worker in workers {
            expired.extend(worker.await??.into_iter().map(|h| h.id()));
        }
        // A capture that lost its lock race to a worker, or a worker that
        // skipped a locked hold, is fine; anything left due is drained now.
        expired.extend(
            store
                .expire_due_holds(at(EXPIRES), usize::MAX)
                .await?
                .into_iter()
                .map(|h| h.id()),
        );
        for (i, capture) in captures.into_iter().enumerate() {
            let n = first + i as u64;
            let worker_won = expired.contains(&hold(n)?);
            match capture.await?? {
                Ok(h) => {
                    assert!(!worker_won, "round {round}: hold {n} captured and expired");
                    assert_eq!(h.status(), HoldStatus::Captured);
                    captured_total += AMOUNT;
                    captures_won += 1;
                }
                Err(LedgerError::HoldNotActive {
                    status: HoldStatus::Expired,
                    ..
                }) => {
                    assert!(worker_won, "round {round}: hold {n} expired by no worker");
                    expiries_won += 1;
                }
                Err(other) => return Err(format!("round {round}, hold {n}: {other}").into()),
            }
        }
        assert_eq!(
            expired.len(),
            {
                let mut unique = expired.clone();
                unique.sort();
                unique.dedup();
                unique.len()
            },
            "round {round}: a hold was expired twice"
        );
    }
    eprintln!("captures won {captures_won}, expiries won {expiries_won}");
    assert!(ended_twice(store.pool()).await?.is_empty());
    assert_eq!(held_and_active(store.pool()).await?, (0, 0));
    assert_eq!(
        balance(&mut store, wallet(0)?).await?,
        100_000 - captured_total
    );
    drop(store);
    db.drop().await?;
    Ok(())
}

/// A worker killed in the middle of a batch (its connection terminated, as
/// when the process dies) is simply run again: every hold ends up expired,
/// each exactly once.
#[tokio::test]
async fn a_worker_killed_mid_batch_expires_no_hold_twice() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    const WALLETS: u64 = 8;
    const HOLDS: u64 = 240;
    let Some((db, mut store, chart)) = setup(WALLETS, 10_000).await? else {
        return Ok(());
    };
    // Placed from each wallet in parallel, to set up faster.
    let mut placing = Vec::new();
    for on in 0..WALLETS {
        let mut store = store.clone();
        let chart = chart.clone();
        placing.push(tokio::spawn(async move {
            for n in (on..HOLDS).step_by(WALLETS as usize) {
                place(&mut store, &chart, n, on, 10, 100).await?;
            }
            Ok::<_, String>(())
        }));
    }
    for task in placing {
        task.await??;
    }
    for on in 0..WALLETS {
        let left = 10_000 - 10 * (HOLDS / WALLETS) as i64;
        assert_eq!(balance(&mut store, wallet(on)?).await?, left);
    }

    let mut conn = db.app().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut conn)
        .await?;
    let worker = tokio::spawn(async move {
        let result = expire_due_holds(
            &mut conn,
            client()?,
            at(100),
            usize::MAX,
            &PostOptions::default(),
        )
        .await;
        Ok::<_, mezgeb_core::IdError>(result.map(|holds| holds.len()))
    });

    let count_expired = async |pool: &PgPool| -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT count(*) FROM holds WHERE status = 'expired'")
            .fetch_one(pool)
            .await
    };
    while count_expired(store.pool()).await? < 20 {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let mut superuser = db.superuser().await?;
    let killed: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&mut superuser)
        .await?;
    superuser.close().await?;
    assert!(killed);
    let outcome = worker.await??;
    assert!(
        outcome.is_err(),
        "the worker finished all {HOLDS} holds before it was killed: {outcome:?}"
    );
    let before_restart = count_expired(store.pool()).await?;
    assert!((20..HOLDS as i64).contains(&before_restart));

    // Restarted, it finishes the rest and nothing more.
    let rest = store.expire_due_holds(at(100), usize::MAX).await?;
    assert_eq!(rest.len() as i64, HOLDS as i64 - before_restart);
    assert_eq!(count_expired(store.pool()).await?, HOLDS as i64);
    assert!(
        store
            .expire_due_holds(at(100), usize::MAX)
            .await?
            .is_empty()
    );

    assert!(ended_twice(store.pool()).await?.is_empty());
    let expiry_keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM idempotency_records WHERE idempotency_key LIKE 'hold-expire:%'",
    )
    .fetch_one(store.pool())
    .await?;
    assert_eq!(expiry_keys, HOLDS as i64);
    assert_eq!(held_and_active(store.pool()).await?, (0, 0));
    for on in 0..WALLETS {
        assert_eq!(balance(&mut store, wallet(on)?).await?, 10_000);
    }
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Acceptance criterion: while holds are placed, captured, released and
/// expired concurrently, every snapshot shows the held accounts holding
/// exactly the sum of the active holds.
#[tokio::test]
async fn held_balance_equals_active_holds_under_concurrency() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    const WALLETS: u64 = 10;
    const HOLDS: u64 = 200;
    let Some((db, store, chart)) = setup(WALLETS, 10_000).await? else {
        return Ok(());
    };
    let clock = Arc::new(AtomicI64::new(0));
    let done = Arc::new(AtomicBool::new(false));

    // Samples INV-18 until the workload is over, on its own connection so
    // that it never waits behind the workload for one.
    let monitor = {
        let mut conn = db.app().await?;
        let done = done.clone();
        tokio::spawn(async move {
            let mut samples = 0u64;
            while !done.load(Ordering::SeqCst) {
                let (held, active) = held_and_active(&mut conn)
                    .await
                    .map_err(|e| e.to_string())?;
                if held != active {
                    return Err(format!("held {held}, active holds {active}"));
                }
                samples += 1;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Ok::<_, String>(samples)
        })
    };
    // Time passes, and two workers expire what is due.
    let ticker = {
        let clock = clock.clone();
        let done = done.clone();
        tokio::spawn(async move {
            while !done.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
                clock.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    let mut workers = Vec::new();
    for _ in 0..2 {
        let store = store.clone();
        let clock = clock.clone();
        let done = done.clone();
        workers.push(tokio::spawn(async move {
            let mut expired = 0;
            while !done.load(Ordering::SeqCst) {
                let now = at(clock.load(Ordering::SeqCst));
                expired += store.expire_due_holds(now, 5).await?.len();
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
            Ok::<_, LedgerError>(expired)
        }));
    }

    let mut rng = Rng(14);
    let mut tasks = Vec::new();
    for n in 0..HOLDS {
        let mut store = store.clone();
        let chart = chart.clone();
        let clock = clock.clone();
        let on = rng.next(WALLETS);
        let amount = 1 + rng.next(100) as i64;
        let ttl = 1 + rng.next(4) as i64;
        let action = rng.next(4);
        let part = 1 + rng.next(amount as u64) as i64;
        tasks.push(tokio::spawn(async move {
            let run = async {
                let now = clock.load(Ordering::SeqCst);
                place(&mut store, &chart, n, on, amount, now + ttl).await?;
                tokio::time::sleep(Duration::from_millis(5 * action)).await;
                let now = at(clock.load(Ordering::SeqCst));
                let result = match action {
                    0 => store.release_hold(hold(n)?, entry(10_000 + n)?).await,
                    1 | 2 => {
                        store
                            .capture_hold(
                                hold(n)?,
                                entry(10_000 + n)?,
                                uuid7(BANK).parse()?,
                                positive(part)?,
                                now,
                            )
                            .await
                    }
                    // Left for the workers.
                    _ => return Ok(()),
                };
                match result {
                    Ok(_)
                    | Err(LedgerError::HoldExpired(_))
                    | Err(LedgerError::HoldNotActive {
                        status: HoldStatus::Expired,
                        ..
                    }) => Ok(()),
                    Err(other) => Err(format!("hold {n}: {other}").into()),
                }
            };
            run.await.map_err(|e: Fail| e.to_string())
        }));
    }
    for task in tasks {
        task.await??;
    }
    done.store(true, Ordering::SeqCst);
    ticker.await?;
    for worker in workers {
        worker.await??;
    }
    let samples = monitor.await??;
    assert!(samples > 0);
    eprintln!("INV-18 held in {samples} snapshots");

    // Afterwards everything is due, and ends.
    store
        .expire_due_holds(at(1_000_000_000), usize::MAX)
        .await?;
    let mut store = store;
    assert!(
        store
            .all_holds()
            .await?
            .iter()
            .all(|h| h.status() != HoldStatus::Active)
    );
    assert!(ended_twice(store.pool()).await?.is_empty());
    assert_eq!(held_and_active(store.pool()).await?, (0, 0));
    // No wallet went below zero on the way.
    for n in 0..WALLETS {
        let left = balance(&mut store, wallet(n)?).await?;
        assert!((0..=10_000).contains(&left), "wallet {n}: {left}");
    }
    drop(store);
    db.drop().await?;
    Ok(())
}

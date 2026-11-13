//! Concurrency tests (step 1.10, ADR-0007): many postings at once, on a
//! multi-threaded runtime with a pool of 20 connections, so they really run
//! in parallel inside PostgreSQL.

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use std::sync::Arc;

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, LedgerError, Money,
    PostedEntry, PostingError,
};
use mezgeb_engine::{PgStore, PostOptions, RetryStats, integrity_report};
use support::{Server, TestDb, TestResult, uuid7};

const POOL: u32 = 20;

fn id(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000_000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// The tests in this file run one at a time. Each one already runs hundreds
/// of postings in parallel against the same accounts, which is what it
/// measures; running several such tests at once on one database only adds
/// load that pushes transactions past their time limits.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A migrated database and a store on a 20-connection pool that counts its
/// retries.
async fn setup() -> Result<Option<(TestDb, PgStore, Arc<RetryStats>)>, Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    let stats = Arc::new(RetryStats::default());
    // These tests measure correctness and deadlock freedom, not latency:
    // on a busy shared test database a task may queue on a hot row for
    // several seconds, so the lock timeout is raised from 5 s to 20 s (still
    // below the 30 s transaction limit on `mezgeb_app`).
    let options = PostOptions {
        stats: Some(stats.clone()),
        lock_timeout: std::time::Duration::from_secs(20),
        ..PostOptions::default()
    };
    let client: ClientId = uuid7(0xc11e).parse()?;
    let store = PgStore::with_options(db.app_pool_of(POOL).await?, client, options);
    Ok(Some((db, store, stats)))
}

/// Posts every draft at the same time, one task each, and returns the
/// results in order.
async fn post_all(
    store: &PgStore,
    drafts: Vec<(EntryId, EntryDraft)>,
) -> Result<Vec<Result<PostedEntry, LedgerError>>, Box<dyn std::error::Error>> {
    let tasks: Vec<_> = drafts
        .into_iter()
        .map(|(id, draft)| {
            let mut store = store.clone();
            tokio::spawn(async move { store.post(id, draft).await })
        })
        .collect();
    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
        results.push(task.await?);
    }
    Ok(results)
}

/// A tiny seeded generator, so every run uses the same "random" workload.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % bound
    }
}

async fn wallet_total(store: &mut PgStore, wallets: &[AccountId]) -> Result<i64, LedgerError> {
    let mut total = 0;
    for wallet in wallets {
        if let Some(account) = store.get_account(*wallet).await? {
            total += account.balance().amount();
        }
    }
    Ok(total)
}

/// 100 tasks each spend 80 from a wallet holding 100: exactly one succeeds,
/// the other 99 are refused for insufficient funds, and 20 is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_hundred_spenders_one_wallet() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    let Some((db, mut store, stats)) = setup().await? else {
        return Ok(());
    };
    let chart = Chart::from_toml(common::CHART)?;
    let (bank, wallet, merchant) = (id(1)?, id(2)?, id(3)?);
    store.open_account(bank, &chart, "1010").await?;
    store
        .open_account(wallet, &chart, "2010:cust-abebe")
        .await?;
    store
        .open_account(merchant, &chart, "2030:merch-coffee")
        .await?;
    store
        .post(
            entry(0)?,
            EntryDraft::default()
                .debit(bank, etb(100))
                .credit(wallet, etb(100)),
        )
        .await?;

    let drafts = (1..=100)
        .map(|n| {
            Ok((
                entry(n)?,
                EntryDraft::default()
                    .debit(wallet, etb(80))
                    .credit(merchant, etb(80)),
            ))
        })
        .collect::<Result<Vec<_>, mezgeb_core::IdError>>()?;
    let results = post_all(&store, drafts).await?;

    let succeeded = results.iter().filter(|r| r.is_ok()).count();
    let refused = results
        .iter()
        .filter(|r| {
            **r == Err(LedgerError::Posting(PostingError::InsufficientFunds {
                line: 0,
            }))
        })
        .count();
    assert_eq!((succeeded, refused), (1, 99), "{results:?}");
    let balance = store.get_account(wallet).await?.map(|a| a.balance());
    assert_eq!(balance, Some(etb(20)));
    assert_eq!(stats.gave_up(), 0);
    assert!(integrity_report(&mut db.app().await?).await?.is_clean());
    drop(store);
    db.drop().await?;
    Ok(())
}

/// 1,000 random transfers among 10 wallets, all at once: money is neither
/// created nor destroyed, nothing deadlocks, and every total equals the sum
/// of its lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_thousand_random_transfers() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    let Some((db, mut store, stats)) = setup().await? else {
        return Ok(());
    };
    let chart = Chart::from_toml(common::CHART)?;
    let bank = id(100)?;
    store.open_account(bank, &chart, "1010").await?;
    let mut wallets = Vec::new();
    for n in 0..10 {
        let wallet = id(n)?;
        store
            .open_account(wallet, &chart, &format!("2010:cust-{n}"))
            .await?;
        store
            .post(
                entry(10_000 + n)?,
                EntryDraft::default()
                    .debit(bank, etb(10_000))
                    .credit(wallet, etb(10_000)),
            )
            .await?;
        wallets.push(wallet);
    }

    let mut rng = SplitMix(42);
    let mut drafts = Vec::new();
    for n in 0..1_000u64 {
        let from = wallets[rng.next(10) as usize];
        let to = wallets[rng.next(10) as usize];
        let other = wallets[rng.next(10) as usize];
        let amount = 1 + rng.next(3_000) as i64;
        let mut draft = EntryDraft {
            // A random wallet may appear twice in a split.
            allow_duplicate_lines: true,
            ..EntryDraft::default()
        };
        // Every third transfer is split between two receivers, and the line
        // order varies, so locks are requested in every possible order.
        draft = if n % 3 == 0 {
            draft
                .credit(to, etb(amount / 2))
                .debit(from, etb(amount))
                .credit(other, etb(amount - amount / 2))
        } else if n % 2 == 0 {
            draft.debit(from, etb(amount)).credit(to, etb(amount))
        } else {
            draft.credit(to, etb(amount)).debit(from, etb(amount))
        };
        drafts.push((entry(n)?, draft));
    }
    let results = post_all(&store, drafts).await?;

    let unexpected: Vec<_> = results
        .iter()
        .filter(|r| {
            !matches!(
                r,
                Ok(_) | Err(LedgerError::Posting(PostingError::InsufficientFunds { .. }))
            )
        })
        .collect();
    assert!(unexpected.is_empty(), "{unexpected:?}");
    assert_eq!(wallet_total(&mut store, &wallets).await?, 100_000);
    assert_eq!(
        stats.deadlock_retries(),
        0,
        "lock ordering prevents deadlocks"
    );
    assert_eq!(stats.gave_up(), 0);
    let report = integrity_report(&mut db.app().await?).await?;
    assert!(report.is_clean(), "{report:?}");
    eprintln!(
        "random transfers: {} posted, {} refused, {} attempts",
        results.iter().filter(|r| r.is_ok()).count(),
        results.iter().filter(|r| r.is_err()).count(),
        stats.attempts()
    );
    drop(store);
    db.drop().await?;
    Ok(())
}

/// The classic deadlock: A→B and B→A at the same time. Locking in ID order
/// means both queue for the same first lock, so no deadlock is possible.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn opposite_transfers_never_deadlock() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    let Some((db, mut store, stats)) = setup().await? else {
        return Ok(());
    };
    let chart = Chart::from_toml(common::CHART)?;
    let (bank, a, b) = (id(1)?, id(2)?, id(3)?);
    store.open_account(bank, &chart, "1010").await?;
    store.open_account(a, &chart, "2010:cust-a").await?;
    store.open_account(b, &chart, "2010:cust-b").await?;
    for (n, wallet) in [a, b].into_iter().enumerate() {
        store
            .post(
                entry(10_000 + n as u64)?,
                EntryDraft::default()
                    .debit(bank, etb(1_000_000))
                    .credit(wallet, etb(1_000_000)),
            )
            .await?;
    }

    let drafts = (0..500u64)
        .map(|n| {
            let (from, to) = if n % 2 == 0 { (a, b) } else { (b, a) };
            Ok((
                entry(n)?,
                EntryDraft::default().debit(from, etb(7)).credit(to, etb(7)),
            ))
        })
        .collect::<Result<Vec<_>, mezgeb_core::IdError>>()?;
    let results = post_all(&store, drafts).await?;

    let errors: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(errors.is_empty(), "{} failed: {:?}", errors.len(), errors);
    assert_eq!(stats.deadlock_retries(), 0);
    assert_eq!(stats.gave_up(), 0);
    assert_eq!(wallet_total(&mut store, &[a, b]).await?, 2_000_000);
    assert!(integrity_report(&mut db.app().await?).await?.is_clean());
    drop(store);
    db.drop().await?;
    Ok(())
}

/// INV-12 under parallel posting: is `recorded_time` strictly increasing in
/// `sequence` order? Measured, not asserted: see the step 1.10 evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn recorded_time_against_sequence() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    let Some((db, mut store, _)) = setup().await? else {
        return Ok(());
    };
    let chart = Chart::from_toml(common::CHART)?;
    let bank = id(1)?;
    store.open_account(bank, &chart, "1010").await?;
    let mut wallets = Vec::new();
    for n in 0..10 {
        let wallet = id(10 + n)?;
        store
            .open_account(wallet, &chart, &format!("2010:cust-{n}"))
            .await?;
        wallets.push(wallet);
    }
    let drafts = (0..1_000u64)
        .map(|n| {
            Ok((
                entry(n)?,
                EntryDraft::default()
                    .debit(bank, etb(5))
                    .credit(wallets[(n % 10) as usize], etb(5)),
            ))
        })
        .collect::<Result<Vec<_>, mezgeb_core::IdError>>()?;
    post_all(&store, drafts).await?;
    let mut conn = db.app().await?;
    let (ties, inversions): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE recorded_time = previous),
                count(*) FILTER (WHERE recorded_time < previous)
           FROM (SELECT recorded_time,
                        lag(recorded_time) OVER (ORDER BY sequence) AS previous
                   FROM journal_entries) ordered",
    )
    .fetch_one(&mut conn)
    .await?;
    eprintln!("INV-12 measurement: 1000 parallel postings, {ties} ties, {inversions} inversions");
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// A posting that waits on a locked account longer than `lock_timeout` gives
/// up with `Busy` and writes nothing; once the lock is gone, the same
/// posting succeeds (step 1.16).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_locked_account_is_busy_after_the_lock_timeout() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    let Some(server) = support::Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let options = PostOptions {
        lock_timeout: std::time::Duration::from_millis(300),
        ..PostOptions::default()
    };
    let mut store = PgStore::with_options(db.app_pool_of(2).await?, client, options);
    let chart = Chart::from_toml(common::CHART)?;
    let (bank, wallet) = (id(1)?, id(2)?);
    store.open_account(bank, &chart, "1010").await?;
    store
        .open_account(wallet, &chart, "2010:cust-abebe")
        .await?;
    let deposit = || {
        EntryDraft::default()
            .debit(bank, etb(100))
            .credit(wallet, etb(100))
    };

    // Another transaction holds the wallet's row lock.
    let mut blocker = db.app().await?;
    sqlx::query("BEGIN").execute(&mut blocker).await?;
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR NO KEY UPDATE")
        .bind(wallet.as_uuid())
        .execute(&mut blocker)
        .await?;

    let started = std::time::Instant::now();
    let refused = store.post(entry(1)?, deposit()).await;
    let waited = started.elapsed();
    assert_eq!(refused.err(), Some(LedgerError::Busy));
    assert!(
        waited >= std::time::Duration::from_millis(300)
            && waited < std::time::Duration::from_secs(5),
        "waited {waited:?}"
    );
    assert_eq!(
        store
            .get_account(wallet)
            .await?
            .map(|a| a.balance().amount()),
        Some(0)
    );

    sqlx::query("ROLLBACK").execute(&mut blocker).await?;
    drop(blocker);
    store.post(entry(1)?, deposit()).await?;
    assert_eq!(
        store
            .get_account(wallet)
            .await?
            .map(|a| a.balance().amount()),
        Some(100)
    );
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Under `SERIALIZABLE`, postings to one wallet at once fail with
/// serialization errors (`40001`). With one attempt they give up with
/// `Contention` and write nothing; with more, each is retried until it
/// posts once (step 1.16).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn serialization_failures_are_retried_or_refused_cleanly() -> TestResult {
    let _turn = ONE_AT_A_TIME.lock().await;
    let Some(server) = support::Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let pool = db.app_pool_of(POOL).await?;
    let store_with = |max_attempts: u32, stats: &Arc<RetryStats>| {
        PgStore::with_options(
            pool.clone(),
            client,
            PostOptions {
                isolation: mezgeb_engine::Isolation::Serializable,
                max_attempts,
                stats: Some(stats.clone()),
                lock_timeout: std::time::Duration::from_secs(20),
                ..PostOptions::default()
            },
        )
    };
    let chart = Chart::from_toml(common::CHART)?;
    let (bank, wallet) = (id(1)?, id(2)?);
    let mut store = PgStore::new(pool.clone(), client);
    store.open_account(bank, &chart, "1010").await?;
    store
        .open_account(wallet, &chart, "2010:cust-abebe")
        .await?;
    let deposits = |from: u64| {
        (from..from + 100)
            .map(|n| {
                Ok((
                    entry(n)?,
                    EntryDraft::default()
                        .debit(bank, etb(1))
                        .credit(wallet, etb(1)),
                ))
            })
            .collect::<Result<Vec<_>, mezgeb_core::IdError>>()
    };

    // One attempt: some give up, and those write nothing.
    let once = Arc::new(RetryStats::default());
    let results = post_all(&store_with(1, &once), deposits(0)?).await?;
    let posted = results.iter().filter(|r| r.is_ok()).count();
    for result in &results {
        match result {
            Ok(_) | Err(LedgerError::Contention(_)) => {}
            Err(other) => return Err(format!("unexpected error: {other}").into()),
        }
    }
    assert!(once.gave_up() > 0, "no posting conflicted");
    assert_eq!(posted as u64 + once.gave_up(), 100);
    assert_eq!(
        store
            .get_account(wallet)
            .await?
            .map(|a| a.balance().amount()),
        Some(posted as i64)
    );

    // Enough attempts: every posting is retried until it posts, once.
    let retried = Arc::new(RetryStats::default());
    let results = post_all(&store_with(100, &retried), deposits(1_000)?).await?;
    for result in results {
        result?;
    }
    assert!(retried.serialization_retries() > 0, "nothing was retried");
    assert_eq!(retried.gave_up(), 0);
    assert_eq!(
        store
            .get_account(wallet)
            .await?
            .map(|a| a.balance().amount()),
        Some(posted as i64 + 100)
    );
    drop(store);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// `lock_timeout` bounds the whole wait for the accounts, not each wait.
/// PostgreSQL applies `lock_timeout` to each lock wait separately, so on a
/// busy account one statement can wait several times in a row: here, first
/// behind another waiter, then behind that waiter's transaction. Each wait
/// is shorter than the timeout, but together they are longer, and the
/// posting must still give up with `Busy` at the timeout (step 1.16).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lock_timeout_bounds_the_whole_wait() -> TestResult {
    use std::time::{Duration, Instant};
    let _turn = ONE_AT_A_TIME.lock().await;
    let Some(server) = support::Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let timeout = Duration::from_millis(400);
    let options = PostOptions {
        lock_timeout: timeout,
        ..PostOptions::default()
    };
    let mut store = PgStore::with_options(db.app_pool_of(2).await?, client, options);
    let chart = Chart::from_toml(common::CHART)?;
    let (bank, wallet) = (id(1)?, id(2)?);
    store.open_account(bank, &chart, "1010").await?;
    store
        .open_account(wallet, &chart, "2010:cust-abebe")
        .await?;
    let lock = "SELECT 1 FROM accounts WHERE id = $1 FOR NO KEY UPDATE";

    // The first holder keeps the wallet locked for 300 ms.
    let mut first = db.app().await?;
    sqlx::query("BEGIN").execute(&mut first).await?;
    sqlx::query(lock)
        .bind(wallet.as_uuid())
        .execute(&mut first)
        .await?;
    // A second transaction queues for the wallet, and holds it for 300 ms
    // once it gets it.
    let mut second = db.app().await?;
    let wallet_id = wallet.as_uuid();
    let queued = tokio::spawn(async move {
        sqlx::query("BEGIN").execute(&mut second).await?;
        sqlx::query(lock)
            .bind(wallet_id)
            .execute(&mut second)
            .await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        sqlx::query("COMMIT").execute(&mut second).await?;
        Ok::<(), sqlx::Error>(())
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The posting queues behind both: about 200 ms behind the second
    // waiter, then 300 ms behind its transaction.
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        sqlx::query("COMMIT").execute(&mut first).await?;
        Ok::<(), sqlx::Error>(())
    });
    let started = Instant::now();
    let result = store
        .post(
            entry(1)?,
            EntryDraft::default()
                .debit(bank, etb(1))
                .credit(wallet, etb(1)),
        )
        .await;
    let waited = started.elapsed();
    release.await??;
    queued.await??;
    assert_eq!(result.err(), Some(LedgerError::Busy), "after {waited:?}");
    assert!(
        waited >= timeout && waited < timeout + Duration::from_millis(250),
        "waited {waited:?}"
    );
    drop(store);
    db.drop().await?;
    Ok(())
}

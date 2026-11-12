//! Idempotency (step 1.11, ADR-0008): the same request has its effect once,
//! however many times and however concurrently it is sent.

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use std::sync::atomic::{AtomicU64, Ordering};

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, IdempotencyKey, Ledger, LedgerError,
    Money, PostedEntry, PostingError,
};
use mezgeb_engine::{
    PgStore, PostOptions, PostRequest, integrity_report, lookup_by_idempotency_key, post_entry,
    post_entry_with, reverse_entry,
};
use sqlx::{Connection, Executor, PgPool};
use support::{Server, TestDb, TestResult, uuid7};

const BANK: u64 = 1;
const ABEBE: u64 = 2;
const HANA: u64 = 3;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn client(n: u64) -> Result<ClientId, mezgeb_core::IdError> {
    uuid7(0xc11e_0000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// A fresh entry ID for every attempt: the ledger assigns IDs, so a retry
/// of the same request arrives with a new one.
fn fresh_entry_id() -> Result<EntryId, mezgeb_core::IdError> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    uuid7(1_000_000 + NEXT.fetch_add(1, Ordering::SeqCst)).parse()
}

fn payment(amount: i64) -> Result<EntryDraft, mezgeb_core::IdError> {
    Ok(EntryDraft::default()
        .debit(account(ABEBE)?, etb(amount))
        .credit(account(HANA)?, etb(amount)))
}

fn request(
    client_n: u64,
    key: &str,
    draft: EntryDraft,
) -> Result<PostRequest, Box<dyn std::error::Error>> {
    Ok(PostRequest {
        entry_id: fresh_entry_id()?,
        client_id: client(client_n)?,
        idempotency_key: IdempotencyKey::new(key)?,
        draft,
        reverses: None,
    })
}

/// A database with the bank and two wallets, Abebe funded with 10,000.
async fn setup() -> Result<Option<(TestDb, PgPool)>, Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    let pool = db.app_pool_of(20).await?;
    let mut store = PgStore::new(pool.clone(), client(0)?);
    let chart = Chart::from_toml(common::CHART)?;
    store.open_account(account(BANK)?, &chart, "1010").await?;
    store
        .open_account(account(ABEBE)?, &chart, "2010:cust-abebe")
        .await?;
    store
        .open_account(account(HANA)?, &chart, "2010:cust-hana")
        .await?;
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

async fn hana_balance(pool: &PgPool) -> Result<i64, Box<dyn std::error::Error>> {
    let mut store = PgStore::new(pool.clone(), client(0)?);
    let account = store
        .get_account(account(HANA)?)
        .await?
        .ok_or("no account")?;
    Ok(account.balance().amount())
}

/// The same request 10 times in a row: one entry, 10 identical responses.
#[tokio::test]
async fn ten_sequential_repeats_post_once() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let before = entry_count(&pool).await?;
    let mut conn = pool.acquire().await?;
    let mut responses = Vec::new();
    for _ in 0..10 {
        responses.push(post_entry(&mut conn, request(1, "pay-42", payment(300)?)?).await?);
    }
    drop(conn);
    assert!(responses.iter().all(|r| *r == responses[0]));
    assert_eq!(entry_count(&pool).await? - before, 1);
    assert_eq!(hana_balance(&pool).await?, 300);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// The same request 50 times at once: one entry, 50 identical responses.
/// The other 49 wait on the key until the first commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fifty_concurrent_repeats_post_once() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let before = entry_count(&pool).await?;
    let tasks = (0..50)
        .map(|_| {
            let pool = pool.clone();
            let request = request(1, "pay-concurrent", payment(250)?)?;
            Ok(tokio::spawn(async move {
                let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
                post_entry(&mut conn, request)
                    .await
                    .map_err(|e| e.to_string())
            }))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let mut responses: Vec<PostedEntry> = Vec::new();
    for task in tasks {
        responses.push(task.await??);
    }
    assert_eq!(responses.len(), 50);
    assert!(responses.iter().all(|r| *r == responses[0]));
    assert_eq!(entry_count(&pool).await? - before, 1);
    assert_eq!(hana_balance(&pool).await?, 250);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// The same key with a different amount: refused, nothing posted.
#[tokio::test]
async fn a_reused_key_with_a_different_request_is_refused() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let mut conn = pool.acquire().await?;
    let original = post_entry(&mut conn, request(1, "pay-7", payment(100)?)?).await?;
    let before = entry_count(&pool).await?;
    let changed = post_entry(&mut conn, request(1, "pay-7", payment(101)?)?).await;
    assert_eq!(changed, Err(LedgerError::IdempotencyKeyReused));
    assert_eq!(entry_count(&pool).await?, before);
    assert_eq!(hana_balance(&pool).await?, 100);

    // The original request is still answered with the original entry.
    let again = post_entry(&mut conn, request(1, "pay-7", payment(100)?)?).await?;
    assert_eq!(again, original);
    drop(conn);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// Failure injection: the posting commits, then the connection is lost
/// before the response reaches the client. The client retries: it gets the
/// original entry back, and no second entry exists.
#[tokio::test]
async fn a_retry_after_a_lost_response_returns_the_original() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let before = entry_count(&pool).await?;
    let lossy = PostOptions {
        lose_response_after_commit: true,
        ..PostOptions::default()
    };

    let first = request(1, "pay-lost", payment(500)?)?;
    let first_id = first.entry_id;
    let mut lost_conn = pool.acquire().await?.detach();
    let lost = post_entry_with(&mut lost_conn, first, &lossy).await;
    assert!(matches!(lost, Err(LedgerError::Storage(_))), "{lost:?}");
    // The connection the response was lost on is gone for good.
    lost_conn.close().await?;
    // The client saw an error, but the posting did commit.
    assert_eq!(entry_count(&pool).await? - before, 1);

    // The client reconnects and retries the same request (with a new entry
    // ID, as a fresh attempt would have).
    let mut conn = pool.acquire().await?;
    let retry = post_entry(&mut conn, request(1, "pay-lost", payment(500)?)?).await?;
    assert_eq!(retry.id(), first_id, "the retry returns the original entry");
    assert_eq!(entry_count(&pool).await? - before, 1, "no second entry");
    assert_eq!(hana_balance(&pool).await?, 500, "the money moved once");

    // It can also just ask what happened.
    let key = IdempotencyKey::new("pay-lost")?;
    let found = lookup_by_idempotency_key(&mut conn, client(1)?, &key).await?;
    assert_eq!(found, Some(retry.clone()));
    eprintln!(
        "lost response: first attempt returned {lost:?}; retry returned entry {} (the original {}); entries added: {}",
        retry.id(),
        first_id,
        entry_count(&pool).await? - before
    );
    assert!(integrity_report(&mut conn).await?.is_clean());
    drop(conn);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// A refused posting writes nothing, not even its key: once the wallet has
/// the money, the same request succeeds.
#[tokio::test]
async fn a_refused_posting_does_not_use_up_its_key() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let mut conn = pool.acquire().await?;
    let too_much = post_entry(&mut conn, request(1, "pay-big", payment(20_000)?)?).await;
    assert_eq!(
        too_much,
        Err(LedgerError::Posting(PostingError::InsufficientFunds {
            line: 0
        }))
    );
    let key = IdempotencyKey::new("pay-big")?;
    assert_eq!(
        lookup_by_idempotency_key(&mut conn, client(1)?, &key).await?,
        None
    );

    let funding = EntryDraft::default()
        .debit(account(BANK)?, etb(10_000))
        .credit(account(ABEBE)?, etb(10_000));
    post_entry(&mut conn, request(1, "fund-2", funding)?).await?;
    let posted = post_entry(&mut conn, request(1, "pay-big", payment(20_000)?)?).await?;
    assert_eq!(
        lookup_by_idempotency_key(&mut conn, client(1)?, &key).await?,
        Some(posted)
    );
    drop(conn);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// Keys belong to a client: two clients may use the same key.
#[tokio::test]
async fn keys_are_scoped_to_their_client() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let mut conn = pool.acquire().await?;
    let a = post_entry(&mut conn, request(1, "order-1", payment(10)?)?).await?;
    let b = post_entry(&mut conn, request(2, "order-1", payment(20)?)?).await?;
    assert_ne!(a.id(), b.id());
    assert_eq!(hana_balance(&pool).await?, 30);
    drop(conn);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// Reversals are idempotent too.
#[tokio::test]
async fn reversals_are_idempotent() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let mut conn = pool.acquire().await?;
    let paid = post_entry(&mut conn, request(1, "pay-r", payment(40)?)?).await?;
    let other = post_entry(&mut conn, request(1, "pay-s", payment(60)?)?).await?;

    let reversal = request(1, "reverse-r", EntryDraft::default())?;
    let first = reverse_entry(&mut conn, paid.id(), reversal).await?;
    let again = request(1, "reverse-r", EntryDraft::default())?;
    assert_eq!(reverse_entry(&mut conn, paid.id(), again).await?, first);

    let wrong = request(1, "reverse-r", EntryDraft::default())?;
    assert_eq!(
        reverse_entry(&mut conn, other.id(), wrong).await,
        Err(LedgerError::IdempotencyKeyReused)
    );
    assert_eq!(hana_balance(&pool).await?, 60);
    drop(conn);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// Idempotency records are append-only, and the service cannot delete them.
#[tokio::test]
async fn idempotency_records_are_append_only() -> TestResult {
    let Some((db, pool)) = setup().await? else {
        return Ok(());
    };
    let mut conn = pool.acquire().await?;
    post_entry(&mut conn, request(1, "pay-x", payment(1)?)?).await?;
    let state = |r: Result<_, sqlx::Error>| {
        r.err().and_then(|e| {
            e.as_database_error()
                .and_then(|d| d.code())
                .map(|c| c.into_owned())
        })
    };
    assert_eq!(
        state(
            conn.execute("UPDATE idempotency_records SET created_at = now()")
                .await
        )
        .as_deref(),
        Some("42501")
    );
    assert_eq!(
        state(conn.execute("DELETE FROM idempotency_records").await).as_deref(),
        Some("42501")
    );
    let mut owner = db.owner().await?;
    assert_eq!(
        state(
            owner
                .execute("UPDATE idempotency_records SET created_at = now()")
                .await
        )
        .as_deref(),
        Some("MZ001")
    );
    drop(owner);
    drop(conn);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

//! Reversals and adjustments on PostgreSQL (step 1.12): corrections never
//! edit history, every correction says who and why, and an adjustment needs
//! two different people (INV-14, INV-25).

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use mezgeb_core::{
    AccountId, ActorId, Authorization, Chart, ClientId, Currency, EntryDraft, EntryId, EntryKind,
    Ledger, LedgerError, Money, PostingError, REASON_CODES, ReasonCode,
};
use mezgeb_engine::PgStore;
use sqlx::{Executor, PgPool};
use support::{Server, TestDb, TestResult, uuid7};

const BANK: u64 = 1;
const ABEBE: u64 = 2;
const HANA: u64 = 3;
const COMPENSATION: u64 = 4;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn auth(
    reason: &str,
    maker: &str,
    checker: Option<&str>,
) -> Result<Authorization, mezgeb_core::NameError> {
    Ok(Authorization {
        reason: ReasonCode::new(reason)?,
        requested_by: ActorId::new(maker)?,
        approved_by: checker.map(ActorId::new).transpose()?,
    })
}

/// The bank, two wallets and the compensation expense account; E01-style
/// deposit of 1,000 to Abebe as entry 1.
async fn setup() -> Result<Option<(TestDb, PgStore, PgPool)>, Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    let pool = db.app_pool().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::new(pool.clone(), client);
    let chart = Chart::from_toml(common::CHART)?;
    store.open_account(account(BANK)?, &chart, "1010").await?;
    store
        .open_account(account(ABEBE)?, &chart, "2010:cust-abebe")
        .await?;
    store
        .open_account(account(HANA)?, &chart, "2010:cust-hana")
        .await?;
    store
        .open_account(account(COMPENSATION)?, &chart, "5010")
        .await?;
    store
        .post(
            entry(1)?,
            EntryDraft::default()
                .debit(account(BANK)?, etb(1_000))
                .credit(account(ABEBE)?, etb(1_000)),
        )
        .await?;
    Ok(Some((db, store, pool)))
}

async fn balances(store: &mut PgStore) -> Result<Vec<(String, i64)>, LedgerError> {
    Ok(store
        .all_accounts()
        .await?
        .iter()
        .map(|a| (a.reference(), a.balance().amount()))
        .collect())
}

/// A hash of an entry's row and of each of its lines, exactly as stored.
async fn row_hashes(pool: &PgPool, id: EntryId) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT md5(row_to_json(e)::text) FROM journal_entries e WHERE e.id = $1
         UNION ALL
         SELECT md5(row_to_json(l)::text)
           FROM (SELECT * FROM journal_lines WHERE entry_id = $1 ORDER BY line_no) l",
    )
    .bind(id.as_uuid())
    .fetch_all(pool)
    .await
}

/// A reversal returns every balance exactly to its prior value, and leaves
/// the original entry's rows byte for byte unchanged.
#[tokio::test]
async fn a_reversal_restores_balances_and_leaves_the_original_untouched() -> TestResult {
    let Some((db, mut store, pool)) = setup().await? else {
        return Ok(());
    };
    let before = balances(&mut store).await?;
    let mistaken = store
        .post(
            entry(2)?,
            EntryDraft::default()
                .debit(account(BANK)?, etb(500))
                .credit(account(ABEBE)?, etb(500)),
        )
        .await?;
    let hashes_before = row_hashes(&pool, mistaken.id()).await?;
    assert_eq!(hashes_before.len(), 3, "the entry row and its two lines");

    let why = auth("booking_error", "ops:1", None)?;
    let reversal = store.reverse(mistaken.id(), entry(3)?, why.clone()).await?;
    assert_eq!(reversal.kind(), EntryKind::Reversal);
    assert_eq!(reversal.reverses(), Some(mistaken.id()));
    assert_eq!(reversal.authorization(), Some(&why));

    assert_eq!(balances(&mut store).await?, before);
    assert_eq!(row_hashes(&pool, mistaken.id()).await?, hashes_before);
    eprintln!("original rows before and after reversal: {hashes_before:?}");
    drop(store);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn reversal_rules_hold() -> TestResult {
    let Some((db, mut store, pool)) = setup().await? else {
        return Ok(());
    };
    let why = auth("booking_error", "ops:1", None)?;
    store.reverse(entry(1)?, entry(2)?, why.clone()).await?;

    // A second reversal of the same entry.
    assert_eq!(
        store.reverse(entry(1)?, entry(3)?, why.clone()).await,
        Err(LedgerError::AlreadyReversed(entry(1)?))
    );
    // A reversal of the reversal.
    assert_eq!(
        store.reverse(entry(2)?, entry(3)?, why.clone()).await,
        Err(LedgerError::CannotReverseReversal(entry(2)?))
    );
    // A reversal with an adjustment's reason.
    let wrong_reason = auth("goodwill_credit", "ops:1", None)?;
    assert_eq!(
        store.reverse(entry(1)?, entry(3)?, wrong_reason).await,
        Err(LedgerError::ReasonNotAllowed("goodwill_credit".into()))
    );
    // A reversal approved by its own requester.
    let self_approved = auth("booking_error", "ops:1", Some("ops:1"))?;
    assert_eq!(
        store.reverse(entry(1)?, entry(3)?, self_approved).await,
        Err(LedgerError::MakerIsChecker)
    );
    drop(store);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// E13's open question: the mistaken deposit was spent before anyone
/// noticed. Reversing it would take the wallet below zero, so it is
/// refused (ADR-0013); the mistake is corrected with an adjustment instead.
#[tokio::test]
async fn a_reversal_that_would_overdraw_is_refused() -> TestResult {
    let Some((db, mut store, pool)) = setup().await? else {
        return Ok(());
    };
    // Abebe spends 600 of the 1,000 deposited in entry 1.
    store
        .post(
            entry(2)?,
            EntryDraft::default()
                .debit(account(ABEBE)?, etb(600))
                .credit(account(HANA)?, etb(600)),
        )
        .await?;
    let before = balances(&mut store).await?;
    assert_eq!(
        store
            .reverse(entry(1)?, entry(3)?, auth("booking_error", "ops:1", None)?)
            .await,
        Err(LedgerError::Posting(PostingError::InsufficientFunds {
            line: 1
        }))
    );
    assert_eq!(balances(&mut store).await?, before, "nothing changed");

    // The correction: an approved adjustment for what is left, and the rest
    // recovered separately (a policy decision, outside the ledger).
    let correction = store
        .post_adjustment(
            entry(3)?,
            EntryDraft::default()
                .debit(account(ABEBE)?, etb(400))
                .credit(account(BANK)?, etb(400)),
            auth("write_off", "ops:1", Some("lead:2"))?,
        )
        .await?;
    assert_eq!(correction.kind(), EntryKind::Adjustment);
    drop(store);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// INV-25: an adjustment needs a reason and two different people; the
/// engine refuses otherwise, and so does the database on its own.
#[tokio::test]
async fn adjustments_need_two_different_people() -> TestResult {
    let Some((db, mut store, pool)) = setup().await? else {
        return Ok(());
    };
    let goodwill = || -> Result<EntryDraft, mezgeb_core::IdError> {
        Ok(EntryDraft::default()
            .debit(account(COMPENSATION)?, etb(10))
            .credit(account(ABEBE)?, etb(10)))
    };
    let before = balances(&mut store).await?;
    for (authorization, error) in [
        (
            auth("goodwill_credit", "agent:1", Some("agent:1"))?,
            LedgerError::MakerIsChecker,
        ),
        (
            auth("goodwill_credit", "agent:1", None)?,
            LedgerError::AuthorizationRequired,
        ),
        (
            auth("booking_error", "agent:1", Some("lead:2"))?,
            LedgerError::ReasonNotAllowed("booking_error".into()),
        ),
    ] {
        assert_eq!(
            store
                .post_adjustment(entry(2)?, goodwill()?, authorization)
                .await,
            Err(error)
        );
    }
    assert_eq!(balances(&mut store).await?, before);

    // E14: proposed by an agent, approved by a lead.
    let approved = auth("goodwill_credit", "agent:1", Some("lead:2"))?;
    let posted = store
        .post_adjustment(entry(2)?, goodwill()?, approved.clone())
        .await?;
    let stored = store.all_entries().await?;
    assert_eq!(stored.last(), Some(&posted));
    assert_eq!(posted.authorization(), Some(&approved));

    // The database refuses a self-approved adjustment and a reason of the
    // wrong kind even when the engine is bypassed.
    let mut conn = db.app().await?;
    let raw = |id: u64, reason: &str, checker: &str| {
        format!(
            "INSERT INTO journal_entries
                 (id, client_id, idempotency_key, kind, reason_code, requested_by, approved_by,
                  period_id)
             VALUES ('{}', '{}', 'raw-{id}', 'adjustment', '{reason}', 'agent:1', '{checker}',
                     (SELECT id FROM periods WHERE status = 'open'))",
            uuid7(9_000 + id),
            uuid7(0xc11e)
        )
    };
    let state = |r: Result<_, sqlx::Error>| {
        r.err().and_then(|e| {
            e.as_database_error()
                .and_then(|d| d.code())
                .map(|c| c.into_owned())
        })
    };
    let same = conn
        .execute(sqlx::AssertSqlSafe(raw(1, "goodwill_credit", "agent:1")))
        .await;
    assert_eq!(state(same).as_deref(), Some("23514"));
    let wrong_kind = conn
        .execute(sqlx::AssertSqlSafe(raw(2, "booking_error", "lead:2")))
        .await;
    assert_eq!(state(wrong_kind).as_deref(), Some("23503"));
    drop(conn);
    drop(store);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

/// The reason codes in the database are exactly those in `mezgeb-core`.
#[tokio::test]
async fn reason_codes_match_the_core_list() -> TestResult {
    let Some((db, store, pool)) = setup().await? else {
        return Ok(());
    };
    let mut stored: Vec<(String, String, String)> =
        sqlx::query_as("SELECT code, entry_kind, description FROM reason_codes")
            .fetch_all(&pool)
            .await?;
    stored.sort();
    let mut expected: Vec<(String, String, String)> = REASON_CODES
        .iter()
        .map(|(code, kind, description)| {
            (
                (*code).to_owned(),
                kind.as_str().to_owned(),
                (*description).to_owned(),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(stored, expected);
    drop(store);
    pool.close().await;
    db.drop().await?;
    Ok(())
}

//! Refunds on PostgreSQL (step 2.04, INV-15): a refund names the entry it
//! refunds, never gives back more than was paid or to anyone else, and
//! cannot meet a reversal of the same entry. Every operation runs on the
//! reference ledger too, and both must agree.

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use mezgeb_core::{
    AccountId, ActorId, Authorization, Chart, ClientId, Currency, EntryDraft, EntryId, EntryKind,
    Ledger, LedgerError, MemLedger, Money, PostedEntry, ReasonCode,
};
use mezgeb_engine::{PgStore, PostOptions, PostRequest, integrity_report, post_entry};
use sqlx::Row;
use support::{Server, TestDb, TestResult, uuid7};

const BANK: u64 = 1;
const ABEBE: u64 = 2;
const HANA: u64 = 3;
const MERCHANT: u64 = 4;
const FEES: u64 = 5;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn booking_error() -> Result<Authorization, mezgeb_core::NameError> {
    Ok(Authorization {
        reason: ReasonCode::new("booking_error")?,
        requested_by: ActorId::new("ops:1")?,
        approved_by: None,
    })
}

/// A refund of `original`: `merchant` back from the merchant, `fee` back
/// from fee revenue, `merchant + fee` to `to`.
fn refund(
    original: EntryId,
    to: u64,
    merchant: i64,
    fee: i64,
) -> Result<EntryDraft, mezgeb_core::IdError> {
    let mut draft = EntryDraft::default();
    if merchant > 0 {
        draft = draft.debit(account(MERCHANT)?, etb(merchant));
    }
    if fee > 0 {
        draft = draft.debit(account(FEES)?, etb(fee));
    }
    draft = draft.credit(account(to)?, etb(merchant + fee));
    draft.refunds = Some(original);
    Ok(draft)
}

/// PostgreSQL and the reference ledger, side by side.
struct Both {
    db: TestDb,
    store: PgStore,
    oracle: MemLedger,
}

impl Both {
    async fn new(server: &Server, pool_size: u32) -> Result<Both, Box<dyn std::error::Error>> {
        let db = server.migrated().await?;
        let client: ClientId = uuid7(0xc11e).parse()?;
        let options = PostOptions {
            lock_timeout: std::time::Duration::from_secs(20),
            ..PostOptions::default()
        };
        let store = PgStore::with_options(db.app_pool_of(pool_size).await?, client, options);
        let mut both = Both {
            db,
            store,
            oracle: MemLedger::new(),
        };
        let chart = Chart::from_toml(common::CHART)?;
        for (n, reference) in [
            (BANK, "1010"),
            (ABEBE, "2010:cust-abebe"),
            (HANA, "2010:cust-hana"),
            (MERCHANT, "2030:merch-coffee"),
            (FEES, "4010"),
        ] {
            both.store
                .open_account(account(n)?, &chart, reference)
                .await?;
            both.oracle.open_account(account(n)?, &chart, reference)?;
        }
        // Abebe deposits 1,000 and pays the merchant 100.00 with a 2.00 fee
        // (E07): entry 2.
        both.post(
            1,
            EntryDraft::default()
                .debit(account(BANK)?, etb(100_000))
                .credit(account(ABEBE)?, etb(100_000)),
        )
        .await??;
        both.post(2, payment()?).await??;
        Ok(both)
    }

    /// Posts on both and checks they agree.
    async fn post(
        &mut self,
        n: u64,
        draft: EntryDraft,
    ) -> Result<Result<PostedEntry, LedgerError>, Box<dyn std::error::Error>> {
        let pg = self.store.post(entry(n)?, draft.clone()).await;
        let mem = Ledger::post(&mut self.oracle, entry(n)?, draft).await;
        assert_eq!(pg, mem, "PostgreSQL and the reference ledger disagree");
        Ok(pg)
    }

    async fn reverse(
        &mut self,
        original: u64,
        n: u64,
    ) -> Result<Result<PostedEntry, LedgerError>, Box<dyn std::error::Error>> {
        let pg = self
            .store
            .reverse(entry(original)?, entry(n)?, booking_error()?)
            .await;
        let mem = Ledger::reverse(
            &mut self.oracle,
            entry(original)?,
            entry(n)?,
            booking_error()?,
        )
        .await;
        assert_eq!(pg, mem, "PostgreSQL and the reference ledger disagree");
        Ok(pg)
    }

    async fn balance(&mut self, n: u64) -> Result<i64, Box<dyn std::error::Error>> {
        let account = self.store.get_account(account(n)?).await?;
        Ok(account.map_or(0, |a| a.balance().amount()))
    }

    async fn finish(self) -> TestResult {
        assert!(
            integrity_report(&mut self.db.app().await?)
                .await?
                .is_clean()
        );
        mezgeb_core::check_invariants(&self.oracle).map_err(|v| v.to_string())?;
        let mut store = self.store;
        assert_eq!(store.all_entries().await?, self.oracle.entries());
        drop(store);
        self.db.drop().await?;
        Ok(())
    }
}

/// E07: Abebe pays 100.00, the merchant gets 98.00, fee revenue 2.00.
fn payment() -> Result<EntryDraft, mezgeb_core::IdError> {
    Ok(EntryDraft::default()
        .debit(account(ABEBE)?, etb(10_000))
        .credit(account(MERCHANT)?, etb(9_800))
        .credit(account(FEES)?, etb(200)))
}

#[tokio::test]
async fn partial_refunds_up_to_the_cap_and_not_one_santim_more() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let mut both = Both::new(&server, 5).await?;
    let original = entry(2)?;

    // E12: 40.00 back, 0.80 of it from the fee.
    let first = both.post(3, refund(original, ABEBE, 3_920, 80)?).await??;
    assert_eq!(first.kind(), EntryKind::Refund);
    assert_eq!(first.refunds(), Some(original));
    // The rest, in two more refunds, exactly to the cap.
    both.post(4, refund(original, ABEBE, 3_000, 0)?).await??;
    both.post(5, refund(original, ABEBE, 2_880, 120)?).await??;
    assert_eq!(both.balance(ABEBE).await?, 100_000);
    assert_eq!(both.balance(MERCHANT).await?, 0);
    assert_eq!(both.balance(FEES).await?, 0);

    // One santim more, from either side, is refused, and changes nothing.
    assert_eq!(
        both.post(6, refund(original, ABEBE, 1, 0)?).await?,
        Err(LedgerError::RefundExceedsOriginal { line: 0 })
    );
    assert_eq!(
        both.post(7, refund(original, ABEBE, 0, 1)?).await?,
        Err(LedgerError::RefundExceedsOriginal { line: 0 })
    );
    let mut conn = both.db.app().await?;
    let tallies: Vec<(i64, i64)> =
        sqlx::query_as("SELECT refunded, limit_amount FROM refund_tallies ORDER BY account_id")
            .fetch_all(&mut conn)
            .await?;
    assert_eq!(tallies, [(10_000, 10_000), (9_800, 9_800), (200, 200)]);
    drop(conn);
    both.finish().await
}

#[tokio::test]
async fn refunds_go_back_only_to_whoever_paid() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let mut both = Both::new(&server, 5).await?;
    let original = entry(2)?;
    // To Hana, who never paid.
    assert_eq!(
        both.post(3, refund(original, HANA, 1_000, 0)?).await?,
        Err(LedgerError::RefundLineNotAllowed { line: 1 })
    );
    // Crediting the merchant: the wrong way.
    let mut wrong = EntryDraft::default()
        .debit(account(ABEBE)?, etb(100))
        .credit(account(MERCHANT)?, etb(100));
    wrong.refunds = Some(original);
    assert_eq!(
        both.post(4, wrong).await?,
        Err(LedgerError::RefundLineNotAllowed { line: 0 })
    );
    // Unknown original.
    assert_eq!(
        both.post(5, refund(entry(99)?, ABEBE, 100, 0)?).await?,
        Err(LedgerError::EntryNotFound(entry(99)?))
    );
    // A deposit can be refunded too: it is a standard entry, and the bank
    // gets back what it put in (here, nothing is left to take from the
    // wallet after the payment, so the posting rules refuse it).
    let mut deposit_refund = EntryDraft::default()
        .debit(account(ABEBE)?, etb(100_000))
        .credit(account(BANK)?, etb(100_000));
    deposit_refund.refunds = Some(entry(1)?);
    assert!(matches!(
        both.post(6, deposit_refund).await?,
        Err(LedgerError::Posting(_))
    ));
    both.finish().await
}

#[tokio::test]
async fn refunds_and_reversals_exclude_each_other() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let mut both = Both::new(&server, 5).await?;
    let payment_entry = entry(2)?;

    // Refund after reversal: rejected.
    both.reverse(2, 3).await??;
    assert_eq!(
        both.post(4, refund(payment_entry, ABEBE, 100, 0)?).await?,
        Err(LedgerError::RefundOfReversedEntry(payment_entry))
    );

    // A second payment, refunded in part: it can no longer be reversed, the
    // refund itself cannot be reversed, and a refund cannot be refunded.
    both.post(5, payment()?).await??;
    let second = entry(5)?;
    both.post(6, refund(second, ABEBE, 980, 20)?).await??;
    assert_eq!(
        both.reverse(5, 7).await?,
        Err(LedgerError::RefundedEntryCannotBeReversed(second))
    );
    assert_eq!(
        both.reverse(6, 8).await?,
        Err(LedgerError::CannotReverseRefund(entry(6)?))
    );
    let mut refund_of_refund = EntryDraft::default()
        .debit(account(ABEBE)?, etb(100))
        .credit(account(MERCHANT)?, etb(100));
    refund_of_refund.refunds = Some(entry(6)?);
    assert_eq!(
        both.post(9, refund_of_refund).await?,
        Err(LedgerError::NotRefundable(entry(6)?))
    );
    // Nor can a reversal be refunded.
    assert_eq!(
        both.post(10, refund(entry(3)?, ABEBE, 100, 0)?).await?,
        Err(LedgerError::NotRefundable(entry(3)?))
    );
    // An adjustment cannot be a refund.
    let adjustment = refund(second, ABEBE, 100, 0)?;
    let approved = Authorization {
        reason: ReasonCode::new("compensation")?,
        requested_by: ActorId::new("ops:1")?,
        approved_by: Some(ActorId::new("ops:2")?),
    };
    let pg = both
        .store
        .post_adjustment(entry(11)?, adjustment.clone(), approved.clone())
        .await;
    let mem = Ledger::post_adjustment(&mut both.oracle, entry(11)?, adjustment, approved).await;
    assert_eq!(pg, Err(LedgerError::RefundAsAdjustment));
    assert_eq!(pg, mem);
    both.finish().await
}

#[tokio::test]
async fn a_repeated_refund_has_its_effect_once() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let both = Both::new(&server, 5).await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let key = mezgeb_core::IdempotencyKey::new("refund-1")?;
    let mut conn = both.db.app().await?;
    let request = PostRequest::new(
        entry(3)?,
        client,
        key.clone(),
        refund(entry(2)?, ABEBE, 3_920, 80)?,
    );
    let first = post_entry(&mut conn, request.clone()).await?;
    let again = post_entry(
        &mut conn,
        PostRequest {
            entry_id: entry(4)?,
            ..request
        },
    )
    .await?;
    assert_eq!(again, first, "a repeat returns the original refund");
    // The same key for a refund of a different entry is a different request.
    let other = PostRequest::new(entry(5)?, client, key, refund(entry(1)?, ABEBE, 3_920, 80)?);
    assert_eq!(
        post_entry(&mut conn, other).await,
        Err(LedgerError::IdempotencyKeyReused)
    );
    let refunded: i64 =
        sqlx::query_scalar("SELECT refunded FROM refund_tallies WHERE account_id = $1")
            .bind(account(ABEBE)?.as_uuid())
            .fetch_one(&mut conn)
            .await?;
    assert_eq!(refunded, 4_000, "the tally counts the refund once");
    drop(conn);
    drop(both.store);
    both.db.drop().await?;
    Ok(())
}

/// 50 partial refunds of one payment, all at once: they queue on the
/// payment's tallies, the ones that fit post, the rest are refused, and the
/// total never exceeds the payment.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fifty_concurrent_partial_refunds_never_exceed_the_cap() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let mut both = Both::new(&server, 20).await?;
    let original = entry(2)?;
    // 50 refunds of 3.00 (2.94 from the merchant, 0.06 from fees): 150.00
    // asked for, 100.00 paid. 33 fit; the 34th would need 102.00.
    let tasks: Vec<_> = (0..50)
        .map(|n| {
            let mut store = both.store.clone();
            let draft = refund(original, ABEBE, 294, 6);
            async move {
                let draft = draft?;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                    store.post(entry(100 + n)?, draft).await,
                )
            }
        })
        .map(tokio::spawn)
        .collect();
    let mut posted = 0;
    let mut refused = 0;
    for task in tasks {
        match task.await?.map_err(|e| e.to_string())? {
            Ok(_) => posted += 1,
            Err(LedgerError::RefundExceedsOriginal { .. }) => refused += 1,
            Err(other) => return Err(format!("unexpected: {other}").into()),
        }
    }
    assert_eq!((posted, refused), (33, 17));
    assert_eq!(both.balance(ABEBE).await?, 90_000 + 33 * 300);
    assert_eq!(both.balance(MERCHANT).await?, 9_800 - 33 * 294);
    assert_eq!(both.balance(FEES).await?, 200 - 33 * 6);
    let mut conn = both.db.app().await?;
    let row =
        sqlx::query("SELECT refunded, limit_amount FROM refund_tallies WHERE account_id = $1")
            .bind(account(ABEBE)?.as_uuid())
            .fetch_one(&mut conn)
            .await?;
    assert_eq!(row.try_get::<i64, _>("refunded")?, 9_900);
    assert_eq!(row.try_get::<i64, _>("limit_amount")?, 10_000);
    assert!(integrity_report(&mut conn).await?.is_clean());
    drop(conn);
    drop(both.store);
    both.db.drop().await?;
    Ok(())
}

/// The database refuses what the engine would never write: a tally going
/// down, a tally above its limit, and an over-refund written straight to
/// the journal.
#[tokio::test]
async fn the_database_enforces_the_refund_rules_on_its_own() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let mut both = Both::new(&server, 5).await?;
    both.post(3, refund(entry(2)?, ABEBE, 980, 20)?).await??;
    let mut conn = both.db.app().await?;
    let sqlstate = |r: Result<sqlx::postgres::PgQueryResult, sqlx::Error>| {
        r.err().and_then(|e| {
            e.as_database_error()
                .and_then(|d| d.code())
                .map(|c| c.into_owned())
        })
    };
    let down = sqlx::query("UPDATE refund_tallies SET refunded = refunded - 1")
        .execute(&mut conn)
        .await;
    assert_eq!(sqlstate(down).as_deref(), Some("MZ007"));
    let over = sqlx::query("UPDATE refund_tallies SET refunded = limit_amount + 1")
        .execute(&mut conn)
        .await;
    assert_eq!(sqlstate(over).as_deref(), Some("23514"));
    let delete = sqlx::query("DELETE FROM refund_tallies")
        .execute(&mut conn)
        .await;
    assert!(sqlstate(delete).is_some(), "tallies cannot be deleted");

    // An over-refund written straight to the journal, bypassing the engine
    // and its tallies: refused at commit.
    let mut tx = sqlx::Connection::begin(&mut conn).await?;
    let refund_id = entry(50)?;
    sqlx::query(
        "INSERT INTO journal_entries
             (id, client_id, idempotency_key, kind, refunds_entry_id, period_id)
         VALUES ($1, $2, 'direct-refund', 'refund', $3, (SELECT id FROM periods WHERE status = 'open'))",
    )
    .bind(refund_id.as_uuid())
    .bind(uuid7(0xc11e).parse::<uuid::Uuid>()?)
    .bind(entry(2)?.as_uuid())
    .execute(&mut *tx)
    .await?;
    for (line, account_n, side, debits, credits) in [
        (0i16, MERCHANT, "debit", 9_800 + 980 + 1, 9_800),
        (1, ABEBE, "credit", 10_000, 100_000 + 1_000 + 9_801),
    ] {
        sqlx::query(
            "INSERT INTO journal_lines
                 (entry_id, line_no, account_id, side, amount, currency, debits_after, credits_after)
             VALUES ($1, $2, $3, $4, 9801, 'ETB', $5, $6)",
        )
        .bind(refund_id.as_uuid())
        .bind(line)
        .bind(account(account_n)?.as_uuid())
        .bind(side)
        .bind(debits)
        .bind(credits)
        .execute(&mut *tx)
        .await?;
    }
    let commit = tx.commit().await;
    let code = commit.err().and_then(|e| {
        e.as_database_error()
            .and_then(|d| d.code())
            .map(|c| c.into_owned())
    });
    assert_eq!(code.as_deref(), Some("MZ007"));
    drop(conn);
    both.finish().await
}

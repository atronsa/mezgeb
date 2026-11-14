//! Posting through the engine on PostgreSQL (step 1.09).

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use std::collections::BTreeMap;

use mezgeb_core::{
    AccountId, AccountStatus, ActorId, Authorization, Chart, ClientId, Currency, EntryDraft,
    EntryId, IdempotencyKey, Ledger, LedgerError, LineDraft, MemLedger, Money, PostingError,
    ReasonCode, StatusError, Timestamp,
};
use mezgeb_engine::{PgStore, PostRequest, integrity_report, post_entry};
use support::{Server, TestResult, uuid7};

fn client() -> Result<ClientId, mezgeb_core::IdError> {
    uuid7(0xc11e).parse()
}

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000 + n).parse()
}

/// A reversal's authorization: a booking error, requested by operations.
fn why() -> Result<Authorization, mezgeb_core::NameError> {
    Ok(Authorization {
        reason: ReasonCode::new("booking_error")?,
        requested_by: ActorId::new("ops:1")?,
        approved_by: None,
    })
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// OB and E01–E20 from the docs post through `PgStore` in order. The final
/// balances equal the trial balance, the history equals the reference
/// ledger's, and the integrity checks are clean.
#[tokio::test]
async fn the_worked_story_posts_on_postgresql() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let mut store = PgStore::new(db.app_pool().await?, client()?);
    let mut oracle = MemLedger::new();
    let chart = Chart::from_toml(common::CHART)?;

    let fixtures = common::fixtures()?;
    assert_eq!(fixtures.len(), 28);
    let mut ids: BTreeMap<String, AccountId> = BTreeMap::new();
    for line in fixtures.iter().flat_map(|f| &f.lines) {
        if !ids.contains_key(&line.reference) {
            let id = account(ids.len() as u64)?;
            store.open_account(id, &chart, &line.reference).await?;
            Ledger::open_account(&mut oracle, id, &chart, &line.reference).await?;
            ids.insert(line.reference.clone(), id);
        }
    }

    for (n, fixture) in fixtures.iter().enumerate() {
        let mut draft = EntryDraft::default();
        for line in &fixture.lines {
            let id = *ids.get(&line.reference).ok_or("unknown reference")?;
            draft.lines.push(LineDraft {
                account: id,
                side: line.side,
                amount: line.amount,
            });
        }
        let id = entry(n as u64)?;
        let posted = store
            .post(id, draft.clone())
            .await
            .map_err(|e| format!("{}: {e}", fixture.label))?;
        let expected = Ledger::post(&mut oracle, id, draft).await?;
        assert_eq!(posted, expected, "{}", fixture.label);
    }

    for (reference, balance) in common::trial_balance()? {
        let id = *ids.get(&reference).ok_or("unknown reference")?;
        let stored = store.get_account(id).await?.ok_or("account missing")?;
        assert_eq!(stored.balance(), balance, "balance of {reference}");
    }
    assert_eq!(
        store.all_entries().await?,
        Ledger::all_entries(&mut oracle).await?
    );
    assert_eq!(
        store.all_accounts().await?,
        Ledger::all_accounts(&mut oracle).await?
    );

    let report = integrity_report(&mut db.app().await?).await?;
    assert!(report.is_clean(), "{report:?}");
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Metadata and event time are stored and read back exactly.
#[tokio::test]
async fn metadata_and_event_time_round_trip() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let chart = Chart::from_toml(common::CHART)?;
    let mut store = PgStore::new(db.app_pool().await?, client()?);
    store.open_account(account(1)?, &chart, "1010").await?;
    store
        .open_account(account(2)?, &chart, "2010:cust-abebe")
        .await?;

    let mut draft = EntryDraft::default()
        .debit(account(1)?, etb(1_000))
        .credit(account(2)?, etb(1_000));
    draft
        .metadata
        .insert("bank_reference".into(), "TT-2026-0042".into());
    draft.metadata.insert("channel".into(), "bank".into());
    draft.event_time = Some(Timestamp::from_micros(1_762_819_200_123_456));
    let posted = store.post(entry(1)?, draft).await?;

    let stored = store.all_entries().await?;
    assert_eq!(stored, vec![posted]);
    assert_eq!(
        stored[0].event_time(),
        Some(Timestamp::from_micros(1_762_819_200_123_456))
    );
    assert_eq!(
        stored[0]
            .metadata()
            .get("bank_reference")
            .map(String::as_str),
        Some("TT-2026-0042")
    );
    drop(store);
    db.drop().await?;
    Ok(())
}

/// A refused posting writes nothing: no entry, no line, no total.
#[tokio::test]
async fn a_refused_posting_writes_nothing() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let chart = Chart::from_toml(common::CHART)?;
    let mut store = PgStore::new(db.app_pool().await?, client()?);
    store.open_account(account(1)?, &chart, "1010").await?;
    store
        .open_account(account(2)?, &chart, "2010:cust-abebe")
        .await?;
    store
        .open_account(account(3)?, &chart, "2010:cust-hana")
        .await?;
    store
        .post(
            entry(1)?,
            EntryDraft::default()
                .debit(account(1)?, etb(500))
                .credit(account(2)?, etb(500)),
        )
        .await?;
    let accounts_before = store.all_accounts().await?;

    let overdraw = EntryDraft::default()
        .debit(account(2)?, etb(501))
        .credit(account(3)?, etb(501));
    assert_eq!(
        store.post(entry(2)?, overdraw).await,
        Err(LedgerError::Posting(PostingError::InsufficientFunds {
            line: 0
        }))
    );
    // Reusing entry 1's idempotency key for a different request (step 1.11).
    let mut conn = db.app().await?;
    let reuse = post_entry(
        &mut conn,
        PostRequest {
            entry_id: entry(3)?,
            client_id: client()?,
            idempotency_key: IdempotencyKey::new(format!("entry-{}", entry(1)?))?,
            draft: EntryDraft::default()
                .debit(account(1)?, etb(1))
                .credit(account(2)?, etb(1)),
            authorization: None,
        },
    )
    .await;
    assert_eq!(reuse, Err(LedgerError::IdempotencyKeyReused));

    assert_eq!(store.all_accounts().await?, accounts_before);
    assert_eq!(store.all_entries().await?.len(), 1);
    assert!(integrity_report(&mut conn).await?.is_clean());
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Account and reversal errors match the reference ledger's.
#[tokio::test]
async fn errors_match_the_reference_ledger() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let chart = Chart::from_toml(common::CHART)?;
    let mut store = PgStore::new(db.app_pool().await?, client()?);
    let mut oracle = MemLedger::new();

    for ledger in [&mut store as &mut dyn LedgerProbe, &mut oracle] {
        ledger.run(&chart).await?;
    }
    drop(store);
    db.drop().await?;
    Ok(())
}

/// The same scripted checks, run against each ledger.
trait LedgerProbe {
    fn run<'a>(
        &'a mut self,
        chart: &'a Chart,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + 'a>>;
}

impl<L: Ledger> LedgerProbe for L {
    fn run<'a>(
        &'a mut self,
        chart: &'a Chart,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + 'a>> {
        Box::pin(async move {
            let bank = account(1)?;
            let wallet = account(2)?;
            self.open_account(bank, chart, "1010").await?;
            self.open_account(wallet, chart, "2010:cust-abebe").await?;
            assert_eq!(
                self.open_account(bank, chart, "4010").await,
                Err(LedgerError::DuplicateAccount(bank))
            );
            assert_eq!(
                self.open_account(account(3)?, chart, "1010").await,
                Err(LedgerError::DuplicateReference("1010".into()))
            );
            assert_eq!(
                self.open_account(account(3)?, chart, "2010").await,
                Err(LedgerError::Chart(
                    mezgeb_core::ChartError::SubAccountRequired(2010)
                ))
            );

            let deposit = EntryDraft::default()
                .debit(bank, etb(1_000))
                .credit(wallet, etb(1_000));
            self.post(entry(1)?, deposit.clone()).await?;
            assert_eq!(
                self.post(entry(1)?, deposit).await,
                Err(LedgerError::DuplicateEntry(entry(1)?))
            );

            let reversal = self.reverse(entry(1)?, entry(2)?, why()?).await?;
            assert_eq!(reversal.reverses(), Some(entry(1)?));
            assert_eq!(
                self.reverse(entry(1)?, entry(3)?, why()?).await,
                Err(LedgerError::AlreadyReversed(entry(1)?))
            );
            assert_eq!(
                self.reverse(entry(2)?, entry(3)?, why()?).await,
                Err(LedgerError::CannotReverseReversal(entry(2)?))
            );
            assert_eq!(
                self.reverse(entry(9)?, entry(3)?, why()?).await,
                Err(LedgerError::EntryNotFound(entry(9)?))
            );
            assert_eq!(
                self.reverse(entry(1)?, entry(2)?, why()?).await,
                Err(LedgerError::DuplicateEntry(entry(2)?))
            );

            self.set_status(wallet, AccountStatus::Closed).await?;
            assert_eq!(
                self.set_status(wallet, AccountStatus::Open).await,
                Err(LedgerError::Status(StatusError::AccountClosed))
            );
            assert_eq!(
                self.set_status(account(9)?, AccountStatus::Frozen).await,
                Err(LedgerError::AccountNotFound(account(9)?))
            );
            assert_eq!(self.get_account(account(9)?).await?, None);
            Ok(())
        })
    }
}

/// Found by the differential test: an entry that debits and credits the
/// same account must post, even when the first line alone would take the
/// account below zero. The engine applies each account's net effect.
#[tokio::test]
async fn the_same_account_on_both_sides_posts() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let chart = Chart::from_toml(common::CHART)?;
    let mut store = PgStore::new(db.app_pool().await?, client()?);
    store.open_account(account(1)?, &chart, "3010").await?;
    let draft = EntryDraft::default()
        .debit(account(1)?, etb(1))
        .credit(account(1)?, etb(1));
    let posted = store.post(entry(1)?, draft).await?;
    let after: Vec<(i64, i64)> = posted
        .lines()
        .iter()
        .map(|l| (l.debits_after(), l.credits_after()))
        .collect();
    assert_eq!(after, vec![(1, 0), (1, 1)]);
    assert!(integrity_report(&mut db.app().await?).await?.is_clean());
    drop(store);
    db.drop().await?;
    Ok(())
}

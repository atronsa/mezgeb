//! What the REST API needs from the engine (step 2.07), on PostgreSQL:
//! idempotent account opening, account lookups, totals at a time, pages of
//! an account's history, entry records and maker-checker adjustment
//! requests.

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use std::collections::BTreeMap;

use mezgeb_core::{
    AccountId, ActorId, Chart, ClientId, Currency, EntryDraft, EntryId, EntryKind, IdempotencyKey,
    Ledger, LedgerError, LineDraft, Money, PostingError, ReasonCode, Side, Timestamp,
};
use mezgeb_engine::{
    AdjustmentStatus, LedgerCursor, NewAdjustment, Opened, Outcome, PgStore, PostOptions,
    account_by_reference, approve_adjustment, entry_record, entry_record_by_key, general_ledger,
    general_ledger_page, get_account, get_adjustment, list_accounts, open_account_keyed,
    reject_adjustment, request_adjustment, totals_at,
};
use support::{Server, TestResult, uuid7};
use uuid::Uuid;

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn id(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn key(text: &str) -> Result<IdempotencyKey, mezgeb_core::IdempotencyKeyError> {
    IdempotencyKey::new(text)
}

#[tokio::test]
async fn accounts_open_once_per_key_and_can_be_found() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let mut conn = db.app().await?;
    let chart = Chart::from_toml(common::CHART)?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let other: ClientId = uuid7(0xc12e).parse()?;

    let first = open_account_keyed(
        &mut conn,
        client,
        &key("open-1")?,
        id(1)?,
        &chart,
        "2010:cust-abebe",
    )
    .await?;
    assert!(matches!(first, Opened::New(_)));
    // The same key again, even with a new ID: the same account.
    let again = open_account_keyed(
        &mut conn,
        client,
        &key("open-1")?,
        id(2)?,
        &chart,
        "2010:cust-abebe",
    )
    .await?;
    assert_eq!(again, Opened::Repeated(first.account().clone()));
    // The same key for another reference: refused.
    assert_eq!(
        open_account_keyed(
            &mut conn,
            client,
            &key("open-1")?,
            id(3)?,
            &chart,
            "2010:cust-hana"
        )
        .await,
        Err(LedgerError::IdempotencyKeyReused)
    );
    // Another key for the same reference: a duplicate.
    assert_eq!(
        open_account_keyed(
            &mut conn,
            client,
            &key("open-2")?,
            id(4)?,
            &chart,
            "2010:cust-abebe"
        )
        .await,
        Err(LedgerError::DuplicateReference("2010:cust-abebe".into()))
    );
    // Keys are per client.
    open_account_keyed(
        &mut conn,
        other,
        &key("open-1")?,
        id(5)?,
        &chart,
        "2010:cust-hana",
    )
    .await?;
    assert!(matches!(
        open_account_keyed(&mut conn, client, &key("open-x")?, id(6)?, &chart, "9999").await,
        Err(LedgerError::Chart(_))
    ));

    let found = account_by_reference(&mut conn, "2010:cust-abebe").await?;
    assert_eq!(found.map(|a| a.id()), Some(id(1)?));
    assert_eq!(
        account_by_reference(&mut conn, "2010:cust-nobody").await?,
        None
    );
    assert_eq!(account_by_reference(&mut conn, "garbage").await?, None);
    assert_eq!(
        get_account(&mut conn, id(5)?).await?.map(|a| a.reference()),
        Some("2010:cust-hana".into())
    );

    let mine = list_accounts(&mut conn, Some(client), None, 10).await?;
    assert_eq!(mine.iter().map(|a| a.id()).collect::<Vec<_>>(), [id(1)?]);
    let all = list_accounts(&mut conn, None, None, 1).await?;
    let rest = list_accounts(&mut conn, None, Some(all[0].id()), 10).await?;
    assert_eq!(all.len() + rest.len(), 2);
    drop(conn);
    db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn totals_at_a_time_pages_and_entry_records() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::new(db.app_pool().await?, client);
    let chart = Chart::from_toml(common::CHART)?;
    store.open_account(id(1)?, &chart, "1010").await?;
    store
        .open_account(id(2)?, &chart, "2010:cust-abebe")
        .await?;
    let mut conn = db.app().await?;
    let now = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT (extract(epoch FROM clock_timestamp()) * 1000000)::bigint",
        )
        .fetch_one(&mut db.app().await?)
        .await
    };
    let before = Timestamp::from_micros(now().await?);
    for n in 0..7u64 {
        let entry: EntryId = uuid7(100 + n).parse()?;
        store
            .post(
                entry,
                EntryDraft::default()
                    .debit(id(1)?, etb(100 * (n as i64 + 1)))
                    .credit(id(2)?, etb(100 * (n as i64 + 1))),
            )
            .await?;
    }
    let after = Timestamp::from_micros(now().await? + 1);
    assert_eq!(totals_at(&mut conn, id(2)?, before).await?, (0, 0));
    assert_eq!(totals_at(&mut conn, id(2)?, after).await?, (0, 2_800));

    // Pages of 3 lines join up to the whole history.
    let wallet = get_account(&mut conn, id(2)?).await?.ok_or("no wallet")?;
    let whole = general_ledger(&mut conn, id(2)?, before, after).await?;
    let mut cursor: Option<LedgerCursor> = None;
    let mut seen = Vec::new();
    let mut opening = Vec::new();
    loop {
        let page = general_ledger_page(&mut conn, &wallet, before, after, cursor, 3).await?;
        opening.push(page.opening_balance);
        seen.extend(page.lines.iter().map(|(line, _)| line.clone()));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, whole.lines);
    assert_eq!(opening, [0, 600, 2_100]);

    let record = entry_record(&mut conn, uuid7(100).parse()?)
        .await?
        .ok_or("no entry")?;
    assert_eq!(record.client, client);
    assert!(record.period_id.is_some());
    assert!(record.recorded_at >= before && record.recorded_at <= after);
    let by_key =
        entry_record_by_key(&mut conn, client, &key(&format!("entry-{}", uuid7(100)))?).await?;
    assert_eq!(by_key, Some(record));
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

fn adjustment(maker: &str) -> Result<NewAdjustment, Box<dyn std::error::Error>> {
    Ok(NewAdjustment {
        reason: ReasonCode::new("goodwill_credit")?,
        lines: vec![
            LineDraft {
                account: id(3)?,
                side: Side::Debit,
                amount: etb(1_000),
            },
            LineDraft {
                account: id(2)?,
                side: Side::Credit,
                amount: etb(1_000),
            },
        ],
        metadata: BTreeMap::from([("ticket".into(), "CS-1".into())]),
        note: Some("outage".into()),
        requested_by: ActorId::new(maker)?,
    })
}

#[tokio::test]
async fn adjustments_need_a_second_person_and_post_once() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::new(db.app_pool().await?, client);
    let chart = Chart::from_toml(common::CHART)?;
    store
        .open_account(id(2)?, &chart, "2010:cust-abebe")
        .await?;
    store.open_account(id(3)?, &chart, "5010").await?;
    let mut conn = db.app().await?;
    let options = PostOptions::default();
    let maker = ActorId::new("ops:maker")?;
    let checker = ActorId::new("ops:checker")?;

    // Requested once per key.
    let request_id = Uuid::now_v7();
    let filed = request_adjustment(
        &mut conn,
        request_id,
        client,
        &key("adj-1")?,
        &adjustment("ops:maker")?,
    )
    .await?;
    assert!(matches!(filed, Outcome::Done(_)));
    assert_eq!(filed.value().status, AdjustmentStatus::Pending);
    let again = request_adjustment(
        &mut conn,
        Uuid::now_v7(),
        client,
        &key("adj-1")?,
        &adjustment("ops:maker")?,
    )
    .await?;
    assert_eq!(again, Outcome::Repeated(filed.value().clone()));
    let mut changed = adjustment("ops:maker")?;
    changed.note = Some("different".into());
    assert_eq!(
        request_adjustment(&mut conn, Uuid::now_v7(), client, &key("adj-1")?, &changed).await,
        Err(LedgerError::IdempotencyKeyReused)
    );
    // A reversal reason, or lines that do not balance, are refused at once.
    let mut reversal = adjustment("ops:maker")?;
    reversal.reason = ReasonCode::new("booking_error")?;
    assert_eq!(
        request_adjustment(&mut conn, Uuid::now_v7(), client, &key("adj-2")?, &reversal).await,
        Err(LedgerError::ReasonNotAllowed("booking_error".into()))
    );
    let mut unbalanced = adjustment("ops:maker")?;
    unbalanced.lines[1].amount = etb(999);
    assert!(matches!(
        request_adjustment(
            &mut conn,
            Uuid::now_v7(),
            client,
            &key("adj-3")?,
            &unbalanced
        )
        .await,
        Err(LedgerError::Posting(PostingError::Unbalanced { .. }))
    ));

    // The maker cannot approve or reject it.
    assert_eq!(
        approve_adjustment(&mut conn, request_id, &maker, &options).await,
        Err(LedgerError::MakerIsChecker)
    );
    assert_eq!(
        reject_adjustment(&mut conn, request_id, &maker, "no").await,
        Err(LedgerError::MakerIsChecker)
    );
    // The checker approves: the adjustment posts with both people.
    let approved = approve_adjustment(&mut conn, request_id, &checker, &options).await?;
    let (request, entry) = approved.value();
    assert_eq!(request.status, AdjustmentStatus::Approved);
    assert_eq!(request.entry, Some(entry.id()));
    assert_eq!(entry.kind(), EntryKind::Adjustment);
    let auth = entry.authorization().ok_or("no authorization")?;
    assert_eq!(
        (
            auth.requested_by.as_str(),
            auth.approved_by.as_ref().map(ActorId::as_str)
        ),
        ("ops:maker", Some("ops:checker"))
    );
    // Approving again returns the same entry; rejecting is too late.
    let repeat = approve_adjustment(&mut conn, request_id, &checker, &options).await?;
    assert!(matches!(repeat, Outcome::Repeated(_)));
    assert_eq!(repeat.value().1.id(), entry.id());
    assert_eq!(
        reject_adjustment(&mut conn, request_id, &checker, "late").await,
        Err(LedgerError::AdjustmentNotPending)
    );
    assert_eq!(
        get_account(&mut conn, id(2)?).await?.map(|a| a.balance()),
        Some(etb(1_000))
    );

    // A rejected request cannot be approved.
    let second = Uuid::now_v7();
    request_adjustment(
        &mut conn,
        second,
        client,
        &key("adj-4")?,
        &adjustment("ops:maker")?,
    )
    .await?;
    let rejected = reject_adjustment(&mut conn, second, &checker, "no evidence").await?;
    assert_eq!(rejected.value().status, AdjustmentStatus::Rejected);
    assert_eq!(
        approve_adjustment(&mut conn, second, &checker, &options).await,
        Err(LedgerError::AdjustmentNotPending)
    );

    // An approval that stopped after "approving" is finished by the next,
    // with the first approver, and posts once.
    let third = Uuid::now_v7();
    request_adjustment(
        &mut conn,
        third,
        client,
        &key("adj-5")?,
        &adjustment("ops:maker")?,
    )
    .await?;
    sqlx::query("UPDATE adjustment_requests SET status = 'approving', decided_by = 'ops:first' WHERE id = $1")
        .bind(third)
        .execute(&mut conn)
        .await?;
    let finished = approve_adjustment(&mut conn, third, &checker, &options).await?;
    let auth = finished
        .value()
        .1
        .authorization()
        .ok_or("no authorization")?;
    assert_eq!(
        auth.approved_by.as_ref().map(ActorId::as_str),
        Some("ops:first")
    );
    let adjustments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM journal_entries WHERE kind = 'adjustment'")
            .fetch_one(&mut conn)
            .await?;
    assert_eq!(adjustments, 2);

    // Lines the ledger refuses post nothing, and the request is pending
    // again: it can still be rejected.
    let refused = Uuid::now_v7();
    let mut overdraw = adjustment("ops:maker")?;
    overdraw.lines[0].account = id(2)?;
    overdraw.lines[1].account = id(3)?;
    overdraw.lines[0].amount = etb(1_000_000);
    overdraw.lines[1].amount = etb(1_000_000);
    request_adjustment(&mut conn, refused, client, &key("adj-6")?, &overdraw).await?;
    assert!(matches!(
        approve_adjustment(&mut conn, refused, &checker, &options).await,
        Err(LedgerError::Posting(PostingError::InsufficientFunds { .. }))
    ));
    let pending = get_adjustment(&mut conn, refused)
        .await?
        .ok_or("request missing")?;
    assert_eq!(
        (pending.status, pending.decided_by),
        (AdjustmentStatus::Pending, None)
    );
    let rejected = reject_adjustment(&mut conn, refused, &checker, "not enough").await?;
    assert_eq!(rejected.value().status, AdjustmentStatus::Rejected);

    // The database refuses going backwards.
    let back = sqlx::query("UPDATE adjustment_requests SET status = 'pending' WHERE id = $1")
        .bind(request_id)
        .execute(&mut conn)
        .await;
    assert!(back.is_err());
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

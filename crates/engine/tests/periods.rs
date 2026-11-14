//! Accounting periods, closing, snapshots and reports on PostgreSQL
//! (step 2.05, INV-10).

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, LedgerError, Money,
    Timestamp,
};
use mezgeb_engine::{
    AsOf, PeriodFinding, PeriodStatus, PgStore, PostOptions, all_periods, begin_close,
    close_period, current_period, finish_close, general_ledger, period_snapshots, trial_balance,
};
use sqlx::{PgConnection, Row};
use support::{Server, TestDb, TestResult, exec, uuid7};

const BANK: u64 = 1;
const MERCHANT: u64 = 2;
const FEES: u64 = 3;
const WALLETS: u64 = 6;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn wallet(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    account(100 + n)
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000_000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// A store with a bank, a merchant, fee revenue and six funded wallets.
async fn setup(
    server: &Server,
    pool: u32,
) -> Result<(TestDb, PgStore), Box<dyn std::error::Error>> {
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let options = PostOptions {
        lock_timeout: Duration::from_secs(20),
        ..PostOptions::default()
    };
    let mut store = PgStore::with_options(db.app_pool_of(pool).await?, client, options);
    let chart = Chart::from_toml(common::CHART)?;
    store.open_account(account(BANK)?, &chart, "1010").await?;
    store
        .open_account(account(MERCHANT)?, &chart, "2030:merch-coffee")
        .await?;
    store.open_account(account(FEES)?, &chart, "4010").await?;
    for n in 0..WALLETS {
        store
            .open_account(wallet(n)?, &chart, &format!("2010:cust-{n}"))
            .await?;
        store
            .post(
                entry(n)?,
                EntryDraft::default()
                    .debit(account(BANK)?, etb(10_000_000))
                    .credit(wallet(n)?, etb(10_000_000)),
            )
            .await?;
    }
    Ok((db, store))
}

/// A tiny seeded generator, so every run is the same.
struct SplitMix(u64);

impl SplitMix {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % bound
    }
}

/// A random payment, transfer or deposit.
fn random_draft(rng: &mut SplitMix) -> Result<EntryDraft, mezgeb_core::IdError> {
    let from = wallet(rng.below(WALLETS))?;
    let amount = 1 + rng.below(5_000) as i64;
    Ok(match rng.below(3) {
        0 => {
            let fee = amount / 50;
            let mut draft = EntryDraft::default()
                .debit(from, etb(amount))
                .credit(account(MERCHANT)?, etb(amount - fee));
            if fee > 0 {
                draft = draft.credit(account(FEES)?, etb(fee));
            }
            draft
        }
        1 => {
            let to = wallet(rng.below(WALLETS))?;
            if to == from {
                EntryDraft::default()
                    .debit(account(BANK)?, etb(amount))
                    .credit(from, etb(amount))
            } else {
                EntryDraft::default()
                    .debit(from, etb(amount))
                    .credit(to, etb(amount))
            }
        }
        _ => EntryDraft::default()
            .debit(account(BANK)?, etb(amount))
            .credit(from, etb(amount)),
    })
}

/// Each account's totals from every line up to `last_sequence`, recomputed
/// from scratch.
async fn recomputed(
    conn: &mut PgConnection,
    last_sequence: u64,
) -> Result<BTreeMap<AccountId, (i64, i64)>, Box<dyn std::error::Error>> {
    let rows = sqlx::query(
        "SELECT l.account_id,
                coalesce(sum(l.amount) FILTER (WHERE l.side = 'debit'), 0)::bigint AS d,
                coalesce(sum(l.amount) FILTER (WHERE l.side = 'credit'), 0)::bigint AS c
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE e.sequence <= $1 GROUP BY l.account_id",
    )
    .bind(i64::try_from(last_sequence)?)
    .fetch_all(&mut *conn)
    .await?;
    let mut totals = BTreeMap::new();
    for row in &rows {
        let id: uuid::Uuid = row.try_get("account_id")?;
        totals.insert(
            AccountId::from_uuid(id)?,
            (row.try_get("d")?, row.try_get("c")?),
        );
    }
    Ok(totals)
}

async fn snapshot_map(
    conn: &mut PgConnection,
    period: i64,
) -> Result<BTreeMap<AccountId, (i64, i64)>, Box<dyn std::error::Error>> {
    Ok(period_snapshots(conn, period)
        .await?
        .into_iter()
        .map(|s| (s.account, (s.debits_total, s.credits_total)))
        .collect())
}

fn sqlstate(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|d| d.code())
        .map(|c| c.into_owned())
}

#[tokio::test]
async fn closing_switches_postings_to_the_next_period() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, mut store) = setup(&server, 5).await?;
    let mut conn = db.app().await?;
    let first = current_period(&mut conn).await?.ok_or("no open period")?;
    assert_eq!(
        (first.label.as_str(), first.status),
        ("initial", PeriodStatus::Open)
    );

    let report = close_period(&mut conn, "day-2").await?;
    assert!(report.is_closed(), "{:?}", report.findings);
    assert_eq!(report.period.id, first.id);
    assert_eq!(report.period.status, PeriodStatus::Closed);
    assert_eq!(report.entries, WALLETS as i64);
    assert_eq!(report.period.last_sequence, Some(WALLETS));
    let next = report.next.ok_or("no next period")?;
    assert_eq!(
        (next.label.as_str(), next.after_sequence),
        ("day-2", WALLETS)
    );

    // New postings go to the new period; the closed one never changes.
    store
        .post(
            entry(100)?,
            EntryDraft::default()
                .debit(wallet(0)?, etb(500))
                .credit(wallet(1)?, etb(500)),
        )
        .await?;
    let period: i64 = sqlx::query_scalar("SELECT period_id FROM journal_entries WHERE id = $1")
        .bind(entry(100)?.as_uuid())
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(period, next.id);
    let periods = all_periods(&mut conn).await?;
    assert_eq!(
        periods.iter().map(|p| p.status).collect::<Vec<_>>(),
        [PeriodStatus::Closed, PeriodStatus::Open]
    );
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn posting_into_a_closed_period_is_refused() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, mut store) = setup(&server, 5).await?;
    let mut conn = db.app().await?;
    let closed = close_period(&mut conn, "day-2").await?.period;

    // The database refuses an entry written straight into the closed period,
    // and a line added to one of its entries.
    let direct = sqlx::query(
        "INSERT INTO journal_entries (id, client_id, idempotency_key, period_id)
         VALUES ($1, $2, 'into-closed', $3)",
    )
    .bind(entry(200)?.as_uuid())
    .bind(uuid7(0xc11e).parse::<uuid::Uuid>()?)
    .bind(closed.id)
    .execute(&mut conn)
    .await;
    assert_eq!(
        direct.as_ref().err().and_then(sqlstate).as_deref(),
        Some("MZ008")
    );
    let old_entry: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM journal_entries WHERE period_id = $1 LIMIT 1")
            .bind(closed.id)
            .fetch_one(&mut conn)
            .await?;
    let line = sqlx::query(
        "INSERT INTO journal_lines
             (entry_id, line_no, account_id, side, amount, currency, debits_after, credits_after)
         VALUES ($1, 9, $2, 'debit', 1, 'ETB', 1, 0)",
    )
    .bind(old_entry)
    .bind(account(BANK)?.as_uuid())
    .execute(&mut conn)
    .await;
    assert!(line.is_err(), "a line was added to a closed period's entry");

    // A closed period cannot reopen; snapshots cannot change; periods
    // cannot be deleted.
    let reopen = sqlx::query("UPDATE periods SET status = 'open' WHERE id = $1")
        .bind(closed.id)
        .execute(&mut conn)
        .await;
    assert!(reopen.is_err());
    let change = sqlx::query("UPDATE period_snapshots SET debits_total = 0")
        .execute(&mut conn)
        .await;
    assert!(change.is_err());
    let delete = sqlx::query("DELETE FROM periods").execute(&mut conn).await;
    assert!(delete.is_err());

    // The engine refuses to post when no period is open.
    let open = current_period(&mut conn).await?.ok_or("no open period")?;
    sqlx::query(
        "UPDATE periods SET status = 'closing', closing_at = now(), last_sequence = after_sequence
          WHERE id = $1",
    )
    .bind(open.id)
    .execute(&mut conn)
    .await?;
    let refused = store
        .post(
            entry(201)?,
            EntryDraft::default()
                .debit(wallet(0)?, etb(1))
                .credit(wallet(1)?, etb(1)),
        )
        .await;
    assert_eq!(refused, Err(LedgerError::NoOpenPeriod));
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn a_snapshot_plus_later_lines_is_the_current_balance() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, mut store) = setup(&server, 5).await?;
    let mut rng = SplitMix(7);
    for n in 0..40 {
        store
            .post(entry(1_000 + n)?, random_draft(&mut rng)?)
            .await?;
    }
    let mut conn = db.app().await?;
    let closed = close_period(&mut conn, "day-2").await?.period;
    for n in 0..40 {
        store
            .post(entry(2_000 + n)?, random_draft(&mut rng)?)
            .await?;
    }
    let snapshot = snapshot_map(&mut conn, closed.id).await?;
    let last = i64::try_from(closed.last_sequence.ok_or("no last sequence")?)?;
    let later = sqlx::query(
        "SELECT l.account_id,
                coalesce(sum(l.amount) FILTER (WHERE l.side = 'debit'), 0)::bigint AS d,
                coalesce(sum(l.amount) FILTER (WHERE l.side = 'credit'), 0)::bigint AS c
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE e.sequence > $1 GROUP BY l.account_id",
    )
    .bind(last)
    .fetch_all(&mut conn)
    .await?;
    let mut expected = snapshot.clone();
    for row in &later {
        let id = AccountId::from_uuid(row.try_get("account_id")?)?;
        let t = expected.entry(id).or_insert((0, 0));
        t.0 += row.try_get::<i64, _>("d")?;
        t.1 += row.try_get::<i64, _>("c")?;
    }
    for account in store.all_accounts().await? {
        let totals = (account.debits_total(), account.credits_total());
        assert_eq!(
            expected.get(&account.id()).copied().unwrap_or((0, 0)),
            totals,
            "{}",
            account.reference()
        );
    }
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// Postings run without pause while periods are closed under them: none
/// lands in a period that has stopped taking postings, every close is clean,
/// and every snapshot equals the recomputation.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn closing_while_postings_are_in_flight() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, store) = setup(&server, 20).await?;
    let writers: Vec<_> = (0..8u64)
        .map(|w| {
            let mut store = store.clone();
            tokio::spawn(async move {
                let mut rng = SplitMix(w);
                for n in 0..60 {
                    let draft = random_draft(&mut rng).map_err(|e| e.to_string())?;
                    let id = entry(10_000 + w * 1_000 + n).map_err(|e| e.to_string())?;
                    store.post(id, draft).await.map_err(|e| e.to_string())?;
                }
                Ok::<(), String>(())
            })
        })
        .collect();
    let mut conn = db.app().await?;
    let mut closed = Vec::new();
    for day in 2..=6 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let report = close_period(&mut conn, &format!("day-{day}")).await?;
        assert!(report.is_closed(), "{:?}", report.findings);
        closed.push(report.period);
    }
    for writer in writers {
        writer.await??;
    }
    let report = close_period(&mut conn, "day-7").await?;
    assert!(report.is_closed(), "{:?}", report.findings);
    closed.push(report.period);

    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM journal_entries")
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(total, WALLETS as i64 + 8 * 60);
    for period in &closed {
        // No entry of the period lies after its end.
        let late: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM journal_entries WHERE period_id = $1 AND sequence > $2",
        )
        .bind(period.id)
        .bind(i64::try_from(period.last_sequence.ok_or("no end")?)?)
        .fetch_one(&mut conn)
        .await?;
        assert_eq!(
            late, 0,
            "an entry landed in {} after it closed",
            period.label
        );
        let last = period.last_sequence.ok_or("no end")?;
        assert_eq!(
            snapshot_map(&mut conn, period.id).await?,
            recomputed(&mut conn, last).await?
        );
    }
    let periods: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE status = 'closed'")
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(periods, 6);
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn a_close_that_stopped_is_finished_and_a_bad_period_stays_closing() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, mut store) = setup(&server, 5).await?;
    let mut conn = db.app().await?;

    // Stopped after the switch: the next close finishes the same period and
    // opens nothing new.
    let closing = begin_close(&mut conn, "day-2").await?;
    assert_eq!(closing.status, PeriodStatus::Closing);
    let report = close_period(&mut conn, "never-opened").await?;
    assert!(report.is_closed());
    assert_eq!(report.period.id, closing.id);
    let labels: Vec<String> = all_periods(&mut conn)
        .await?
        .into_iter()
        .map(|p| p.label)
        .collect();
    assert_eq!(labels, ["initial", "day-2"]);

    // A period whose journal was tampered with does not close.
    store
        .post(
            entry(300)?,
            EntryDraft::default()
                .debit(wallet(0)?, etb(100))
                .credit(wallet(1)?, etb(100)),
        )
        .await?;
    let closing = begin_close(&mut conn, "day-3").await?;
    let mut superuser = db.superuser().await?;
    let victim = entry(300)?.as_uuid();
    exec(
        &mut superuser,
        format!(
            "SET session_replication_role = replica;
             INSERT INTO journal_lines
                 (entry_id, line_no, account_id, side, amount, currency, debits_after, credits_after)
             VALUES ('{victim}', 7, '{}', 'debit', 5, 'ETB', 0, 0)",
            account(FEES)?.as_uuid()
        ),
    )
    .await?;
    // The entry no longer balances, and fee revenue's totals from the lines
    // no longer match its stored running totals.
    let report = finish_close(&mut conn).await?;
    assert!(!report.is_closed());
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f, PeriodFinding::EntryUnbalanced { .. }))
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f, PeriodFinding::TotalsDisagree { .. }))
    );
    assert_eq!(report.period.id, closing.id);
    assert_eq!(report.period.status, PeriodStatus::Closing);
    assert!(period_snapshots(&mut conn, closing.id).await?.is_empty());
    drop(superuser);
    drop(conn);
    drop(store);
    db.drop_unverified().await?;
    Ok(())
}

#[tokio::test]
async fn reports_balance_and_agree_with_each_other() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, mut store) = setup(&server, 5).await?;
    let mut rng = SplitMix(11);
    for n in 0..30 {
        store
            .post(entry(1_000 + n)?, random_draft(&mut rng)?)
            .await?;
    }
    let mut conn = db.app().await?;
    let midpoint: i64 =
        sqlx::query_scalar("SELECT (extract(epoch FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&mut conn)
            .await?;
    let closed = close_period(&mut conn, "day-2").await?.period;
    for n in 0..30 {
        store
            .post(entry(2_000 + n)?, random_draft(&mut rng)?)
            .await?;
    }

    let now = trial_balance(&mut conn, AsOf::Now).await?;
    let at_close = trial_balance(&mut conn, AsOf::Period(closed.id)).await?;
    let at_midpoint =
        trial_balance(&mut conn, AsOf::Time(Timestamp::from_micros(midpoint))).await?;
    for tb in [&now, &at_close, &at_midpoint] {
        assert!(tb.is_balanced(), "{:?}", tb.totals());
    }
    // Everything before the midpoint was in the closed period.
    assert_eq!(at_midpoint.rows, at_close.rows);
    assert_ne!(now.rows, at_close.rows);
    assert_eq!(
        trial_balance(&mut conn, AsOf::Period(closed.id + 1)).await,
        Err(LedgerError::PeriodNotClosed(closed.id + 1))
    );

    // The general ledger of a wallet over everything: opening 0, each line
    // moves the balance, and the closing balance is the account's balance.
    let far = Timestamp::from_micros(i64::MAX / 2);
    let gl = general_ledger(&mut conn, wallet(0)?, Timestamp::from_micros(0), far).await?;
    assert_eq!(gl.opening_balance, 0);
    assert_eq!(
        gl.closing_balance,
        now.row(wallet(0)?).map_or(0, |r| r.balance())
    );
    // Split at the midpoint: the second half opens where the first closes.
    let mid = Timestamp::from_micros(midpoint);
    let first = general_ledger(&mut conn, wallet(0)?, Timestamp::from_micros(0), mid).await?;
    let second = general_ledger(&mut conn, wallet(0)?, mid, far).await?;
    assert_eq!(second.opening_balance, first.closing_balance);
    assert_eq!(first.lines.len() + second.lines.len(), gl.lines.len());
    assert_eq!(second.closing_balance, gl.closing_balance);
    assert!(gl.lines.windows(2).all(|w| w[0].sequence <= w[1].sequence));
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

/// The acceptance criterion: 30 simulated days, each with random postings
/// and a close. After every close, the snapshot equals the full
/// recomputation from every line, and the trial balance balances.
#[tokio::test]
async fn thirty_days_of_snapshots_match_the_full_recomputation() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, mut store) = setup(&server, 5).await?;
    let mut conn = db.app().await?;
    let mut rng = SplitMix(30);
    let mut n = 0;
    for day in 1..=30u64 {
        let postings = 5 + rng.below(30);
        for _ in 0..postings {
            n += 1;
            store
                .post(entry(10_000 + n)?, random_draft(&mut rng)?)
                .await?;
        }
        let report = close_period(&mut conn, &format!("day-{}", day + 1)).await?;
        assert!(report.is_closed(), "day {day}: {:?}", report.findings);
        let last = report.period.last_sequence.ok_or("no end")?;
        assert_eq!(
            snapshot_map(&mut conn, report.period.id).await?,
            recomputed(&mut conn, last).await?,
            "day {day}"
        );
        let tb = trial_balance(&mut conn, AsOf::Period(report.period.id)).await?;
        assert!(tb.is_balanced(), "day {day}");
        assert_eq!(
            tb.rows,
            trial_balance(&mut conn, AsOf::Now).await?.rows,
            "day {day}"
        );
    }
    let closed: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE status = 'closed'")
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(closed, 30);
    drop(conn);
    drop(store);
    db.drop().await?;
    Ok(())
}

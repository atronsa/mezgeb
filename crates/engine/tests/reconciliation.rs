//! Reconciliation on PostgreSQL (step 2.15, ADR-0020): statement import,
//! the external run with every planted break, resolution through suspense
//! entries, cache reconciliation, and the guards on the tables.

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use std::collections::BTreeMap;

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, Money, Timestamp,
};
use mezgeb_engine::recon::{
    BreakFilter, BreakKind, BreakStatus, CachedBalance, Mapping, MatchOptions, ReconBreak,
    ReconError, RunKind, explain_break, get_break, import_statement, list_breaks, list_runs,
    parse_statement, reconcile_account, reconcile_cache, resolve_break,
};
use mezgeb_engine::{PgStore, create_client};
use sqlx::PgConnection;
use support::{Server, TestResult, uuid7};

const DAY: i64 = 86_400_000_000;
/// 2026-11-01, in days since 1970-01-01.
const D0: i64 = 20_393;
/// The templates that may resolve a break, as the service passes them.
const TEMPLATES: &[&str] = &["deposit_unmatched.v1", "bank_debit_unmatched.v1"];

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn at_day(day: i64) -> Timestamp {
    Timestamp::from_micros((D0 + day) * DAY + 3_600_000_000)
}

/// A ledger with a client, the accounts the tests use, and a way to post.
struct Books {
    store: PgStore,
    client: ClientId,
    ids: BTreeMap<&'static str, AccountId>,
    posted: u64,
}

impl Books {
    async fn new(db: &support::TestDb) -> Result<Books, Box<dyn std::error::Error>> {
        let client: ClientId = uuid7(0xc11e).parse()?;
        let mut admin = db.admin().await?;
        create_client(&mut admin, client, "acme", 1_000, 1_000).await?;
        let mut store = PgStore::new(db.app_pool().await?, client);
        let chart = Chart::from_toml(common::CHART)?;
        let mut ids = BTreeMap::new();
        for (n, reference) in [
            "1010",
            "2010:cust-a",
            "9010:sus-1",
            "9010:sus-2",
            "9040:sbd-1",
            "9040:sbd-2",
        ]
        .into_iter()
        .enumerate()
        {
            let id: AccountId = uuid7(n as u64 + 1).parse()?;
            store.open_account(id, &chart, reference).await?;
            ids.insert(reference, id);
        }
        Ok(Books {
            store,
            client,
            ids,
            posted: 0,
        })
    }

    fn id(&self, reference: &str) -> AccountId {
        self.ids[reference]
    }

    /// Posts Dr `debit` / Cr `credit`, dated `day`, with metadata.
    async fn post(
        &mut self,
        debit: &str,
        credit: &str,
        minor: i64,
        day: i64,
        metadata: &[(&str, &str)],
    ) -> Result<EntryId, Box<dyn std::error::Error>> {
        self.posted += 1;
        let id: EntryId = uuid7(10_000 + self.posted).parse()?;
        let mut draft = EntryDraft::default()
            .debit(self.id(debit), etb(minor))
            .credit(self.id(credit), etb(minor));
        draft.metadata = metadata
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        draft.event_time = Some(at_day(day));
        self.store.post(id, draft).await?;
        Ok(id)
    }
}

fn statement(rows: &[(&str, i64, &str, &str)]) -> Result<String, Box<dyn std::error::Error>> {
    let mut text = String::from("line_id,booking_date,amount,currency,reference\n");
    for (id, day, amount, reference) in rows {
        let date = mezgeb_engine::recon::format_date(i32::try_from(D0 + day)?);
        text.push_str(&format!("{id},{date},{amount},ETB,{reference}\n"));
    }
    Ok(text)
}

async fn import(
    conn: &mut PgConnection,
    account: AccountId,
    text: &str,
    day: i64,
) -> Result<mezgeb_engine::recon::ImportReport, Box<dyn std::error::Error>> {
    let lines = parse_statement(text, &Mapping::mezgeb(), Currency::ETB)?;
    Ok(import_statement(conn, account, &lines, "test.csv mezgeb-csv-v1", at_day(day)).await?)
}

async fn open_breaks(
    conn: &mut PgConnection,
) -> Result<Vec<ReconBreak>, Box<dyn std::error::Error>> {
    Ok(list_breaks(
        conn,
        &BreakFilter {
            status: Some(BreakStatus::Open),
            limit: 1000,
            ..BreakFilter::default()
        },
    )
    .await?)
}

fn kinds(breaks: &[ReconBreak]) -> Vec<BreakKind> {
    let mut kinds: Vec<BreakKind> = breaks.iter().map(|b| b.kind).collect();
    kinds.sort();
    kinds
}

fn find(breaks: &[ReconBreak], kind: BreakKind) -> Result<ReconBreak, String> {
    breaks
        .iter()
        .find(|b| b.kind == kind)
        .cloned()
        .ok_or_else(|| format!("no {kind} break"))
}

/// The acceptance criterion: each planted break (a missing statement line,
/// an amount mismatch, a duplicate statement line, a movement the ledger
/// lacks) is detected and classified, with no false match; each is then
/// resolved by a suspense entry, and the account reconciles clean.
#[tokio::test]
async fn every_planted_break_is_found_and_resolved_by_suspense_entries() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let mut books = Books::new(&db).await?;
    let mut conn = db.verifier().await?;
    let bank = books.id("1010");

    // The ledger: four deposits.
    books
        .post("1010", "2010:cust-a", 100_000, 0, &[("bank_ref", "B1")])
        .await?;
    books
        .post("1010", "2010:cust-a", 50_000, 1, &[("payment_ref", "HANA")])
        .await?;
    let mismatched = books
        .post("1010", "2010:cust-a", 20_000, 2, &[("bank_ref", "B3")])
        .await?;
    let unseen = books
        .post("1010", "2010:cust-a", 30_000, 2, &[("bank_ref", "B4")])
        .await?;

    // The bank: B1 exactly; HANA by the rule, a day later; B3 for 190 not
    // 200; no B4; B5, which the ledger lacks; and B6, repeating B1's date,
    // amount and reference under its own ID. B1 is printed twice.
    let text = statement(&[
        ("B1", 0, "1000.00", "ABEBE"),
        ("B1", 0, "1000.00", "ABEBE"),
        ("B2", 2, "500.00", "HANA"),
        ("B3", 2, "190.00", ""),
        ("B5", 4, "70.00", "UNKNOWN"),
        ("B6", 0, "1000.00", "ABEBE"),
    ])?;
    let report = import(&mut conn, bank, &text, 4).await?;
    assert_eq!((report.lines, report.imported, report.skipped), (6, 5, 1));

    let run = reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(10)).await?;
    assert_eq!(run.kind, RunKind::External);
    assert_eq!((run.matched, run.pending, run.breaks_new), (2, 0, 4));
    let breaks = open_breaks(&mut conn).await?;
    assert_eq!(
        kinds(&breaks),
        [
            BreakKind::MissingInLedger,
            BreakKind::MissingInStatement,
            BreakKind::AmountMismatch,
            BreakKind::DuplicateStatementLine,
        ]
    );
    let missing_statement = find(&breaks, BreakKind::MissingInStatement)?;
    assert_eq!(missing_statement.line, Some((unseen.as_uuid(), 0)));
    assert_eq!(missing_statement.difference_minor, Some(-30_000));
    let mismatch = find(&breaks, BreakKind::AmountMismatch)?;
    assert_eq!(mismatch.line, Some((mismatched.as_uuid(), 0)));
    assert_eq!(
        (
            mismatch.other_minor,
            mismatch.ledger_minor,
            mismatch.difference_minor
        ),
        (Some(19_000), Some(20_000), Some(-1_000))
    );
    assert_eq!(
        find(&breaks, BreakKind::MissingInLedger)?.difference_minor,
        Some(7_000)
    );
    assert_eq!(
        find(&breaks, BreakKind::DuplicateStatementLine)?.difference_minor,
        Some(100_000)
    );

    // Running again finds the same, and raises nothing twice.
    let again = reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(10)).await?;
    assert_eq!(
        (again.matched, again.breaks_new, again.breaks_open),
        (0, 0, 4)
    );

    // A money break is never explained, only resolved by an entry.
    let refused = explain_break(
        &mut conn,
        mismatch.id,
        "looks fine",
        books.client,
        at_day(10),
    )
    .await;
    assert!(
        matches!(refused, Err(ReconError::Conflict(_))),
        "{refused:?}"
    );

    // The bank has 10 less than the ledger for B3: Dr 9040 / Cr 1010.
    let fix = books
        .post(
            "9040:sbd-1",
            "1010",
            1_000,
            10,
            &[("template", "bank_debit_unmatched.v1")],
        )
        .await?;
    // A wrong template, amount or direction is refused.
    let wrong_template = books
        .post(
            "9040:sbd-2",
            "1010",
            30_000,
            10,
            &[("template", "p2p_transfer.v1")],
        )
        .await?;
    for (entry, against) in [(wrong_template, mismatch.id), (fix, missing_statement.id)] {
        let refused = resolve_break(
            &mut conn,
            against,
            entry,
            books.client,
            TEMPLATES,
            at_day(10),
        )
        .await;
        assert!(
            matches!(refused, Err(ReconError::Conflict(_))),
            "{refused:?}"
        );
    }
    let resolved = resolve_break(
        &mut conn,
        mismatch.id,
        fix,
        books.client,
        TEMPLATES,
        at_day(10),
    )
    .await?;
    assert_eq!(resolved.status, BreakStatus::Resolved);
    assert_eq!(resolved.resolution, Some((fix.as_uuid(), 1)));
    // The same entry never resolves a second break.
    let reused = resolve_break(
        &mut conn,
        missing_statement.id,
        fix,
        books.client,
        TEMPLATES,
        at_day(10),
    )
    .await;
    assert!(matches!(reused, Err(ReconError::Conflict(_))), "{reused:?}");

    // B4 never reached the bank: the ledger has 300 more.
    let parked = books
        .post(
            "9040:sbd-2",
            "1010",
            30_000,
            10,
            &[("template", "bank_debit_unmatched.v1")],
        )
        .await?;
    resolve_break(
        &mut conn,
        missing_statement.id,
        parked,
        books.client,
        TEMPLATES,
        at_day(10),
    )
    .await?;
    // B5 and B6 are in the bank and not the ledger: Dr 1010 / Cr 9010.
    for (kind, item, minor) in [
        (BreakKind::MissingInLedger, "9010:sus-1", 7_000),
        (BreakKind::DuplicateStatementLine, "9010:sus-2", 100_000),
    ] {
        let entry = books
            .post(
                "1010",
                item,
                minor,
                10,
                &[("template", "deposit_unmatched.v1")],
            )
            .await?;
        let open = find(&breaks, kind)?;
        let done = resolve_break(
            &mut conn,
            open.id,
            entry,
            books.client,
            TEMPLATES,
            at_day(10),
        )
        .await?;
        assert_eq!(done.status, BreakStatus::Resolved, "{kind}");
    }

    // Now 1010 agrees with the bank, and the next run finds nothing: the
    // suspense lines are accounted for by the breaks they resolved.
    let clean = reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(11)).await?;
    assert_eq!((clean.breaks_new, clean.breaks_open), (0, 0), "{clean:?}");
    let store_balance: i64 =
        sqlx::query_scalar("SELECT debits_total - credits_total FROM accounts WHERE id = $1")
            .bind(bank.as_uuid())
            .fetch_one(&mut conn)
            .await?;
    // The bank's total, less the entry booked with the wrong template,
    // which is dated after the last statement: pending until one covers it.
    assert_eq!(
        store_balance,
        100_000 + 50_000 + 19_000 + 7_000 + 100_000 - 30_000
    );
    assert_eq!(clean.pending, 1);

    drop(conn);
    db.drop().await?;
    Ok(())
}

/// A statement line that arrives late clears the break it caused; a young
/// line waits; the same ID with other content is a conflict, explained by
/// an operator.
#[tokio::test]
async fn late_lines_clear_their_breaks_and_conflicts_are_explained() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let mut books = Books::new(&db).await?;
    let mut conn = db.verifier().await?;
    let bank = books.id("1010");

    books
        .post("1010", "2010:cust-a", 10_000, 0, &[("bank_ref", "B1")])
        .await?;
    books
        .post("1010", "2010:cust-a", 20_000, 1, &[("bank_ref", "B2")])
        .await?;
    import(&mut conn, bank, &statement(&[("B0", 0, "5.00", "")])?, 5).await?;
    let first = reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(1)).await?;
    // B0 is a day old: pending. The ledger lines are inside the window of a
    // statement that ends on day 0: pending too.
    assert_eq!((first.breaks_new, first.pending), (0, 3));

    let second = reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(5)).await?;
    assert_eq!(second.breaks_new, 1, "B0: in the bank, not the ledger");

    // The next statement covers days 0 to 4: B1 is there, B2 is not.
    import(
        &mut conn,
        bank,
        &statement(&[("B1", 0, "100.00", ""), ("B9", 4, "1.00", "")])?,
        5,
    )
    .await?;
    let third = reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(5)).await?;
    assert_eq!(third.matched, 1);
    let open = open_breaks(&mut conn).await?;
    assert_eq!(
        kinds(&open),
        [BreakKind::MissingInLedger, BreakKind::MissingInStatement]
    );

    // B2 arrives late, and the client books B0 with its bank_ref.
    import(&mut conn, bank, &statement(&[("B2", 3, "200.00", "")])?, 6).await?;
    books
        .post("1010", "2010:cust-a", 500, 0, &[("bank_ref", "B0")])
        .await?;
    let fourth = reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(6)).await?;
    assert_eq!(
        (fourth.matched, fourth.breaks_cleared),
        (2, 2),
        "{fourth:?}"
    );
    // B9 is now two days old, and in the bank only.
    assert_eq!(
        kinds(&open_breaks(&mut conn).await?),
        [BreakKind::MissingInLedger]
    );
    let cleared = get_break(&mut conn, open[0].id).await?;
    assert_eq!(cleared.status, BreakStatus::Cleared);
    assert_eq!(cleared.cleared_by_run, Some(fourth.id));

    // The same B1 with another amount: a conflict; the stored line stays.
    let report = import(&mut conn, bank, &statement(&[("B1", 0, "101.00", "")])?, 7).await?;
    assert_eq!((report.imported, report.conflicts), (0, 1));
    let conflict = find(&open_breaks(&mut conn).await?, BreakKind::StatementConflict)?;
    let explained = explain_break(
        &mut conn,
        conflict.id,
        "the bank corrected its export; the first copy is right",
        books.client,
        at_day(7),
    )
    .await?;
    assert_eq!(explained.status, BreakStatus::Explained);
    let twice = explain_break(&mut conn, conflict.id, "again", books.client, at_day(7)).await;
    assert!(matches!(
        twice,
        Err(ReconError::Closed(BreakStatus::Explained))
    ));

    let runs = list_runs(&mut conn, Some(RunKind::Import), 10).await?;
    assert_eq!(runs.len(), 4);
    assert!(runs.iter().all(|r| r.account == Some(bank)));

    drop(conn);
    db.drop().await?;
    Ok(())
}

/// A client's cache is compared at the sequence it reflects; the ledger
/// wins, and a later agreeing call clears the break.
#[tokio::test]
async fn a_stale_cache_is_a_break_until_it_agrees() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let mut books = Books::new(&db).await?;
    let mut conn = db.app().await?;
    let wallet = books.id("2010:cust-a");

    books.post("1010", "2010:cust-a", 10_000, 0, &[]).await?;
    let head: i64 = sqlx::query_scalar("SELECT max(sequence) FROM journal_entries")
        .fetch_one(&mut conn)
        .await?;
    // Posted after the client's cache was computed.
    books.post("1010", "2010:cust-a", 5_000, 0, &[]).await?;

    let cached = |balance_minor, as_of_sequence| CachedBalance {
        account: wallet,
        balance_minor,
        as_of_sequence,
    };
    // At the cache's sequence, 100.00 is right: no break.
    let agree = reconcile_cache(
        &mut conn,
        books.client,
        &[cached(10_000, Some(head))],
        at_day(1),
    )
    .await?;
    assert!(agree.results[0].agrees);
    assert_eq!(agree.run.breaks_new, 0);
    // Now, it is stale: the ledger says 150.00.
    let stale =
        reconcile_cache(&mut conn, books.client, &[cached(10_000, None)], at_day(1)).await?;
    assert_eq!(stale.results[0].ledger_minor, 15_000);
    assert!(!stale.results[0].agrees);
    let open = open_breaks(&mut conn).await?;
    assert_eq!(kinds(&open), [BreakKind::StaleCache]);
    assert_eq!(open[0].difference_minor, Some(-5_000));
    // Refreshed from the ledger, the cache agrees and the break clears.
    let refreshed =
        reconcile_cache(&mut conn, books.client, &[cached(15_000, None)], at_day(1)).await?;
    assert_eq!(refreshed.run.breaks_cleared, 1);
    assert!(open_breaks(&mut conn).await?.is_empty());

    // Refusals.
    for (balances, what) in [
        (vec![], "none"),
        (vec![cached(1, None), cached(1, None)], "twice"),
        (vec![cached(1, Some(head + 100))], "ahead"),
    ] {
        let refused = reconcile_cache(&mut conn, books.client, &balances, at_day(1)).await;
        assert!(
            matches!(refused, Err(ReconError::Invalid(_))),
            "{what}: {refused:?}"
        );
    }
    let unknown = CachedBalance {
        account: uuid7(999).parse()?,
        balance_minor: 0,
        as_of_sequence: None,
    };
    let refused = reconcile_cache(&mut conn, books.client, &[unknown], at_day(1)).await;
    assert!(
        matches!(refused, Err(ReconError::NotFound(_))),
        "{refused:?}"
    );

    drop(conn);
    db.drop().await?;
    Ok(())
}

/// Runs never change, a statement line is never edited or deleted and its
/// match is set once, and a closed break never moves again, whoever asks.
#[tokio::test]
async fn reconciliation_history_cannot_change() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let mut books = Books::new(&db).await?;
    let mut conn = db.verifier().await?;
    let bank = books.id("1010");
    books
        .post("1010", "2010:cust-a", 10_000, 0, &[("bank_ref", "B1")])
        .await?;
    import(
        &mut conn,
        bank,
        &statement(&[("B1", 0, "100.00", ""), ("B2", 0, "1.00", "")])?,
        5,
    )
    .await?;
    reconcile_account(&mut conn, bank, MatchOptions::default(), at_day(5)).await?;

    let mut owner = db.owner().await?;
    for sql in [
        "UPDATE recon_runs SET items = items + 1",
        "DELETE FROM recon_runs",
        "UPDATE recon_items SET amount_minor = 1 WHERE line_ref = 'B2'",
        "UPDATE recon_items SET match_rule = NULL WHERE line_ref = 'B1'",
        "DELETE FROM recon_items",
        "UPDATE recon_breaks SET detail = 'changed'",
        "DELETE FROM recon_breaks",
    ] {
        let error = sqlx::query(sql).execute(&mut owner).await.expect_err(sql);
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("MZ001"),
            "{sql}"
        );
    }
    // The service may not touch the ledger through reconciliation.
    let mut app = db.app().await?;
    let denied = sqlx::query("DELETE FROM recon_items")
        .execute(&mut app)
        .await;
    assert!(denied.is_err());

    drop((conn, owner, app));
    db.drop().await?;
    Ok(())
}

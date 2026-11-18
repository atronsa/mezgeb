//! Reconciliation at scale (step 2.15): `mezgeb-bench --recon ITEMS`.
//!
//! Builds a ledger whose 1010 holds `ITEMS / 2` lines, and a statement of
//! as many lines, so that `ITEMS` items in all are reconciled. Most ledger
//! lines are in entries of 50 lines each, matched by the rule pass (amount,
//! reference and date); the rest are one-line entries with the bank's line
//! ID, matched exactly. Four kinds of break are planted, 100 of each: a
//! ledger line missing from the statement, a statement line missing from
//! the ledger, another amount, and a statement line repeating another.
//!
//! Then it times parsing and importing the statement, and the run, and
//! checks the acceptance criterion: every planted break is found, of its
//! kind, nothing else is, and every match pairs the two lines of the same
//! movement (zero false matches). The ledger is written through the engine,
//! as the service writes it.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use mezgeb_core::{Chart, Currency, EntryDraft, Ledger, Timestamp};
use mezgeb_engine::recon::{
    BreakFilter, BreakKind, BreakStatus, Mapping, MatchOptions, format_date, import_statement,
    list_breaks, parse_statement, reconcile_account,
};
use mezgeb_engine::{MIGRATOR, PgStore, PostOptions, post_entry_with};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, Row};
use uuid::Uuid;

use crate::{CHART, Result, bank, client, etb, request, role, wallet};

/// Lines per rule-matched entry.
const PER_ENTRY: usize = 50;
/// Planted breaks of each kind.
const PLANTED: usize = 100;
/// The statements cover 30 days.
const DAYS: usize = 30;
/// 2026-11-01, in days since 1970-01-01.
const D0: i64 = 20_393;
const DAY_MICROS: i64 = 86_400_000_000;

/// One ledger line, and the statement line it should match.
struct Movement {
    /// The entry and line number in the ledger.
    entry: usize,
    line_no: i16,
    line_ref: String,
    day: usize,
    amount: i64,
    reference: String,
}

pub async fn run(database_url: &str, items: usize) -> Result<bool> {
    let ledger_lines = items / 2;
    if ledger_lines < 10 * PLANTED + PER_ENTRY {
        return Err(format!(
            "--recon needs at least {} items",
            2 * (10 * PLANTED + PER_ENTRY)
        )
        .into());
    }
    let admin: PgConnectOptions = database_url.parse()?;
    let database = format!("mezgeb_bench_recon_{}", std::process::id());
    let mut superuser = PgConnection::connect_with(&admin).await?;
    superuser
        .execute(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database} OWNER mezgeb_owner"
        )))
        .await?;
    let result = measure(&admin, &database, ledger_lines).await;
    match &result {
        Ok(_) => {
            superuser
                .execute(sqlx::AssertSqlSafe(format!(
                    "DROP DATABASE {database} WITH (FORCE)"
                )))
                .await?;
        }
        Err(_) => eprintln!("kept database {database} for inspection"),
    }
    superuser.close().await?;
    result
}

async fn measure(admin: &PgConnectOptions, database: &str, ledger_lines: usize) -> Result<bool> {
    let mut owner = PgConnection::connect_with(&role(admin, database, "mezgeb_owner")).await?;
    MIGRATOR.run(&mut owner).await?;
    owner.close().await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(role(admin, database, "mezgeb_app"))
        .await?;
    crate::signer()
        .register(&mut *pool.acquire().await?)
        .await?;
    let chart = Chart::from_toml(CHART)?;
    let mut store = PgStore::new(pool.clone(), client()?);
    store.open_account(bank()?, &chart, "1010").await?;
    store
        .open_account(wallet(0)?, &chart, "2010:cust-0")
        .await?;

    // The ledger: exact entries first (one line each, 2% of the lines),
    // then entries of PER_ENTRY lines matched by the rule.
    let exact_entries = (ledger_lines / 50).max(4 * PLANTED);
    let rule_entries = (ledger_lines - exact_entries) / PER_ENTRY;
    let mut movements = Vec::with_capacity(ledger_lines);
    let mut entry_ids = Vec::with_capacity(exact_entries + rule_entries);
    let started = Instant::now();
    let mut conn = pool.acquire().await?;
    let options = PostOptions {
        lock_timeout: Duration::from_secs(25),
        ..PostOptions::default()
    };
    for k in 0..exact_entries + rule_entries {
        let day = k % DAYS;
        let mut draft = EntryDraft {
            event_time: Some(Timestamp::from_micros(
                (D0 + i64::try_from(day)?) * DAY_MICROS + 3_600_000_000,
            )),
            allow_duplicate_lines: true,
            ..EntryDraft::default()
        };
        let mut total = 0;
        if k < exact_entries {
            // A unique amount, so no two exact lines share a key.
            let amount = 10_000 + i64::try_from(k)?;
            let line_ref = format!("X{k}");
            draft.metadata.insert("bank_ref".into(), line_ref.clone());
            draft = draft.debit(bank()?, etb(amount));
            total = amount;
            movements.push(Movement {
                entry: k,
                line_no: 0,
                line_ref,
                day,
                amount,
                reference: String::new(),
            });
        } else {
            let reference = format!("P{k}");
            draft
                .metadata
                .insert("payment_ref".into(), reference.clone());
            for j in 0..PER_ENTRY {
                let amount = 100 + i64::try_from(j)?;
                draft = draft.debit(bank()?, etb(amount));
                total += amount;
                movements.push(Movement {
                    entry: k,
                    line_no: i16::try_from(j)?,
                    line_ref: format!("S{k}-{j}"),
                    day,
                    amount,
                    reference: reference.clone(),
                });
            }
        }
        draft = draft.credit(wallet(0)?, etb(total));
        let posted = request(format!("recon-{k}"), draft)?;
        entry_ids.push(posted.entry_id.as_uuid());
        post_entry_with(&mut conn, posted, &options).await?;
    }
    drop(conn);
    pool.close().await;
    println!(
        "ledger: {} lines in 1010, {} entries, written in {:.1} s",
        movements.len(),
        entry_ids.len(),
        started.elapsed().as_secs_f64()
    );

    // The statement, with the planted breaks. Exact movements 0..100 have
    // another amount on the statement; the statement repeats exact
    // movements 100..200 under new IDs; the last 100 rule movements are
    // missing from it; and 100 lines are in it only.
    let mut csv = String::from("line_id,booking_date,amount,currency,reference\n");
    let mut row = |line_ref: &str, day: usize, minor: i64, reference: &str| -> Result<()> {
        let date = format_date(i32::try_from(D0 + i64::try_from(day)?)?);
        writeln!(
            csv,
            "{line_ref},{date},{}.{:02},ETB,{reference}",
            minor / 100,
            minor % 100
        )?;
        Ok(())
    };
    let missing_from_statement = movements.len() - PLANTED..movements.len();
    for (n, m) in movements.iter().enumerate() {
        if missing_from_statement.contains(&n) {
            continue;
        }
        let amount = if n < PLANTED { m.amount + 1 } else { m.amount };
        row(&m.line_ref, m.day, amount, &m.reference)?;
        if (PLANTED..2 * PLANTED).contains(&n) {
            row(&format!("D{n}"), m.day, m.amount, &m.reference)?;
        }
    }
    // Dated after the ledger's last day, so the statement covers every
    // ledger line by more than the grace period.
    for n in 0..PLANTED {
        row(&format!("E{n}"), DAYS + 3, 777, &format!("EXTRA{n}"))?;
    }

    let mut verifier =
        PgConnection::connect_with(&role(admin, database, "mezgeb_verifier")).await?;
    let account = bank()?;
    let today = Timestamp::from_micros((D0 + i64::try_from(DAYS)? + 5) * DAY_MICROS);
    let started = Instant::now();
    let lines = parse_statement(&csv, &Mapping::mezgeb(), Currency::ETB)?;
    let parsed = started.elapsed();
    let report = import_statement(&mut verifier, account, &lines, "bench", today).await?;
    let imported = started.elapsed();
    let started = Instant::now();
    let run = reconcile_account(&mut verifier, account, MatchOptions::default(), today).await?;
    let reconciled = started.elapsed();

    // Every match pairs a statement line with its own movement.
    let truth: HashMap<&str, (Uuid, i16)> = movements
        .iter()
        .map(|m| (m.line_ref.as_str(), (entry_ids[m.entry], m.line_no)))
        .collect();
    let mut false_matches = 0;
    for r in sqlx::query(
        "SELECT line_ref, matched_entry_id, matched_line_no FROM recon_items
          WHERE account_id = $1 AND match_rule IS NOT NULL",
    )
    .bind(account.as_uuid())
    .fetch_all(&mut verifier)
    .await?
    {
        let line_ref: String = r.try_get("line_ref")?;
        let matched = (
            r.try_get::<Uuid, _>("matched_entry_id")?,
            r.try_get::<i16, _>("matched_line_no")?,
        );
        if truth.get(line_ref.as_str()) != Some(&matched) {
            false_matches += 1;
        }
    }
    let breaks = list_breaks(
        &mut verifier,
        &BreakFilter {
            status: Some(BreakStatus::Open),
            limit: 1000,
            ..BreakFilter::default()
        },
    )
    .await?;
    verifier.close().await?;
    let count = |kind: BreakKind| breaks.iter().filter(|b| b.kind == kind).count();
    let found = [
        BreakKind::MissingInStatement,
        BreakKind::MissingInLedger,
        BreakKind::AmountMismatch,
        BreakKind::DuplicateStatementLine,
    ]
    .map(count);

    let total_items = report.imported + movements.len();
    println!(
        "statement: {} lines parsed in {:.1} s, imported in {:.1} s",
        report.lines,
        parsed.as_secs_f64(),
        (imported - parsed).as_secs_f64()
    );
    println!(
        "run: {total_items} items ({} statement lines, {} ledger lines) reconciled in {:.1} s \
         ({:.0} items/s); recorded runtime {:.1} s",
        report.imported,
        movements.len(),
        reconciled.as_secs_f64(),
        total_items as f64 / reconciled.as_secs_f64(),
        run.runtime_micros as f64 / 1e6
    );
    println!(
        "matched {} · breaks: missing in statement {}, missing in ledger {}, amount mismatch {}, \
         duplicate {} (each {PLANTED} planted) · other breaks {} · false matches {false_matches}",
        run.matched,
        found[0],
        found[1],
        found[2],
        found[3],
        breaks.len() - found.iter().sum::<usize>()
    );
    let expected_matches = movements.len() - 2 * PLANTED;
    let ok = found == [PLANTED; 4]
        && breaks.len() == 4 * PLANTED
        && false_matches == 0
        && usize::try_from(run.matched)? == expected_matches;
    println!(
        "{}",
        if ok {
            "PASS: every planted break found and classified, no other break, zero false matches"
        } else {
            "FAIL"
        }
    );
    Ok(ok)
}

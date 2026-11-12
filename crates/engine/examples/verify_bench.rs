//! Times verification on a large ledger (step 1.13).
//!
//! ```sh
//! cargo run --release --example verify_bench -- postgres://postgres:postgres@localhost:55432/postgres 500000
//! ```
//!
//! The URL is a superuser's: the benchmark creates its own database, and
//! bulk-loads valid entries with SQL (triggers off, for speed): each entry
//! debits the bank and credits one of 1,000 wallets, with correct running
//! totals on every line. Then it times:
//!
//! 1. a full verification of the first batch;
//! 2. an incremental verification after a second batch of the same size;
//! 3. an incremental verification with nothing new;
//! 4. a full verification of both batches.

use std::time::Instant;

use mezgeb_engine::{MIGRATOR, Scope, VerifyOptions, verify};
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, Executor, PgConnection};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const WALLETS: i64 = 1_000;

/// Loads entries `start + 1 ..= start + count`, two lines each.
async fn load(conn: &mut PgConnection, start: i64, count: i64) -> Result<()> {
    let sql = format!(
        "SET session_replication_role = replica;
         CREATE TEMP TABLE batch AS
           SELECT k,
                  ('00000000-0000-7000-9000-' || lpad(to_hex(k), 12, '0'))::uuid AS entry_id,
                  100 + k % 997 AS amount,
                  k % {WALLETS} AS wallet
             FROM generate_series({first}, {last}) k;
         INSERT INTO journal_entries (id, sequence, client_id, idempotency_key, recorded_time)
           OVERRIDING SYSTEM VALUE
           SELECT entry_id, k, '00000000-0000-7000-8000-00000000c11e', 'bench-' || k,
                  now() - interval '1 hour'
             FROM batch;
         INSERT INTO journal_lines
             (entry_id, line_no, account_id, side, amount, currency, debits_after, credits_after)
           SELECT b.entry_id, 0, '00000000-0000-7000-8000-000000000001', 'debit', b.amount, 'ETB',
                  bank.debits_total + sum(b.amount) OVER (ORDER BY b.k), 0
             FROM batch b,
                  (SELECT debits_total FROM accounts
                    WHERE id = '00000000-0000-7000-8000-000000000001') bank
           UNION ALL
           SELECT b.entry_id, 1, w.id, 'credit', b.amount, 'ETB',
                  0, w.credits_total + sum(b.amount) OVER (PARTITION BY b.wallet ORDER BY b.k)
             FROM batch b
             JOIN accounts w
               ON w.id = ('00000000-0000-7000-8000-' || lpad(to_hex(100 + b.wallet), 12, '0'))::uuid;
         UPDATE accounts SET debits_total = debits_total + (SELECT sum(amount) FROM batch)
          WHERE id = '00000000-0000-7000-8000-000000000001';
         UPDATE accounts a SET credits_total = credits_total + s.total
           FROM (SELECT wallet, sum(amount) AS total FROM batch GROUP BY wallet) s
          WHERE a.id = ('00000000-0000-7000-8000-' || lpad(to_hex(100 + s.wallet), 12, '0'))::uuid;
         SELECT setval(pg_get_serial_sequence('journal_entries', 'sequence'), {last});
         DROP TABLE batch;
         ANALYZE journal_entries;
         ANALYZE journal_lines;",
        first = start + 1,
        last = start + count,
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(conn)
        .await?;
    Ok(())
}

async fn timed(conn: &mut PgConnection, label: &str, scope: Scope) -> Result<()> {
    let options = VerifyOptions {
        scope,
        settle: std::time::Duration::ZERO,
        record: true,
    };
    let started = Instant::now();
    let report = verify(conn, &options).await?;
    println!(
        "| {label} | {} | {} | {:.2} s | {} |",
        report.from_sequence,
        report.lines_checked,
        started.elapsed().as_secs_f64(),
        if report.is_clean() { "clean" } else { "DIRTY" }
    );
    if !report.is_clean() {
        print!("{report}");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let admin: PgConnectOptions = args
        .next()
        .ok_or("usage: verify_bench <superuser-url> [entries-per-batch]")?
        .parse()?;
    let batch: i64 = args
        .next()
        .map(|n| n.parse())
        .transpose()?
        .unwrap_or(500_000);

    let name = format!("mezgeb_bench_{}", std::process::id());
    let mut server = PgConnection::connect_with(&admin).await?;
    server
        .execute(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {name} OWNER mezgeb_owner"
        )))
        .await?;
    let db = |user: &str| admin.clone().database(&name).username(user).password(user);
    let mut owner = PgConnection::connect_with(&db("mezgeb_owner")).await?;
    MIGRATOR.run(&mut owner).await?;
    owner.close().await?;

    let mut superuser = PgConnection::connect_with(&admin.clone().database(&name)).await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO accounts (id, code, name, account_type, currency)
           VALUES ('00000000-0000-7000-8000-000000000001', 1010, 'Bank', 'asset', 'ETB');
         INSERT INTO accounts (id, code, sub_account, name, account_type, currency)
           SELECT ('00000000-0000-7000-8000-' || lpad(to_hex(100 + w), 12, '0'))::uuid,
                  2010, 'cust-' || w, 'Wallet', 'liability', 'ETB'
             FROM generate_series(0, {last}) w;",
        last = WALLETS - 1
    )))
    .execute(&mut superuser)
    .await?;

    let mut verifier = PgConnection::connect_with(&db("mezgeb_verifier")).await?;
    println!("| Run | From entry | Lines checked | Time | Result |");
    println!("|---|---:|---:|---:|---|");

    let started = Instant::now();
    load(&mut superuser, 0, batch).await?;
    eprintln!(
        "loaded {} lines in {:.1} s",
        2 * batch,
        started.elapsed().as_secs_f64()
    );
    timed(&mut verifier, "Full, first batch", Scope::Full).await?;

    let started = Instant::now();
    load(&mut superuser, batch, batch).await?;
    eprintln!(
        "loaded {} more lines in {:.1} s",
        2 * batch,
        started.elapsed().as_secs_f64()
    );
    timed(
        &mut verifier,
        "Incremental, second batch only",
        Scope::Incremental,
    )
    .await?;
    timed(
        &mut verifier,
        "Incremental, nothing new",
        Scope::Incremental,
    )
    .await?;
    timed(&mut verifier, "Full, both batches", Scope::Full).await?;

    verifier.close().await?;
    superuser.close().await?;
    server
        .execute(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {name} WITH (FORCE)"
        )))
        .await?;
    Ok(())
}

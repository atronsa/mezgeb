//! Crash test for step 1.09: kill a process in the middle of posting and
//! prove that no partial entry is ever left behind.
//!
//! ```sh
//! export OWNER_URL=postgres://mezgeb_owner:mezgeb_owner@localhost:55432/mezgeb
//! export APP_URL=postgres://mezgeb_app:mezgeb_app@localhost:55432/mezgeb
//! cargo build --release --example crash_writer
//! target/release/examples/crash_writer setup "$OWNER_URL"
//! timeout -s KILL 5 target/release/examples/crash_writer write "$APP_URL"   # killed mid-run
//! target/release/examples/crash_writer check "$APP_URL"                     # must be clean
//! ```
//!
//! `write` posts three-line entries as fast as it can until it is killed.
//! `check` runs the integrity queries and exits with status 1 if any fails.

use std::process::ExitCode;

use mezgeb_core::{AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, Money};
use mezgeb_engine::{MIGRATOR, PgStore, integrity_report};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection};

const CHART: &str = r#"
name = "Crash test chart"
currency = "ETB"

[[accounts]]
code = 1010
name = "Bank"
type = "asset"
kind = "system"
status = "active"

[[accounts]]
code = 2010
name = "Customer wallets"
type = "liability"
kind = "control"
prefix = "cust"
status = "active"

[[accounts]]
code = 4010
name = "Fee revenue"
type = "revenue"
kind = "system"
status = "active"
"#;

const REFERENCES: [&str; 7] = [
    "1010",
    "4010",
    "2010:cust-1",
    "2010:cust-2",
    "2010:cust-3",
    "2010:cust-4",
    "2010:cust-5",
];

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn account(n: usize) -> Result<AccountId> {
    Ok(uuid7(n as u64 + 1).parse()?)
}

async fn setup(url: &str) -> Result<()> {
    let mut conn = PgConnection::connect(url).await?;
    MIGRATOR.run(&mut conn).await?;
    println!("migrated");
    Ok(())
}

async fn write(url: &str) -> Result<()> {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::new(pool.clone(), client);
    let chart = Chart::from_toml(CHART)?;
    for (n, reference) in REFERENCES.iter().enumerate() {
        if store.get_account(account(n)?).await?.is_none() {
            store.open_account(account(n)?, &chart, reference).await?;
        }
    }
    let start: i64 = sqlx::query_scalar("SELECT count(*) FROM journal_entries")
        .fetch_one(&pool)
        .await?;
    let mut next = u64::try_from(start)? + 1;
    loop {
        // Deposit to a wallet with a fee: Dr bank, Cr wallet, Cr revenue.
        let amount = 1_000 + (next % 997) as i64;
        let fee = amount / 50;
        let wallet = 2 + (next % 5) as usize;
        let draft = EntryDraft::default()
            .debit(account(0)?, Money::from_minor(amount, Currency::ETB))
            .credit(
                account(wallet)?,
                Money::from_minor(amount - fee, Currency::ETB),
            )
            .credit(account(1)?, Money::from_minor(fee, Currency::ETB));
        let id: EntryId = uuid7(1_000_000 + next).parse()?;
        store.post(id, draft).await?;
        if next % 500 == 0 {
            println!("posted {next} entries");
        }
        next += 1;
    }
}

async fn check(url: &str) -> Result<bool> {
    let mut conn = PgConnection::connect(url).await?;
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM journal_entries")
        .fetch_one(&mut conn)
        .await?;
    let report = integrity_report(&mut conn).await?;
    println!("{entries} entries; {report:?}");
    Ok(report.is_clean())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(command), Some(url)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: crash_writer <setup|write|check> <database-url>");
        return ExitCode::from(2);
    };
    let outcome = match command.as_str() {
        "setup" => setup(url).await.map(|()| true),
        "write" => write(url).await.map(|()| true),
        "check" => check(url).await,
        _ => Err("unknown command".into()),
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("integrity check FAILED");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

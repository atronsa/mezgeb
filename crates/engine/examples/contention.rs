//! Measures the concurrency schemes from ADR-0007 on the same workloads
//! (step 1.10), and prints a table for the ADR's evidence section.
//!
//! ```sh
//! docker compose up -d
//! cargo run --release --example contention -- postgres://postgres:postgres@localhost:55432/postgres
//! ```
//!
//! The URL is a superuser's: each run creates and drops its own database.
//!
//! Schemes:
//! - `RC + locks`: READ COMMITTED, `FOR NO KEY UPDATE` in ID order (chosen);
//! - `SER + locks`: SERIALIZABLE with the same locks;
//! - `SER, no locks`: SERIALIZABLE alone;
//! - `RC, no locks`: READ COMMITTED alone (unsafe: balances checked while
//!   other transactions change them; only the database CHECK stops overdraft).
//!
//! Workloads:
//! - `hot wallet`: 100 spends of 80 from one wallet holding 100;
//! - `random`: 1,000 transfers among 10 wallets;
//! - `hot merchant`: 1,000 payments from 10 wallets into one merchant.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, LedgerError, Money,
    PostingError,
};
use mezgeb_engine::{Isolation, MIGRATOR, PgStore, PostOptions, RetryStats, integrity_report};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const CHART: &str = r#"
name = "Contention chart"
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
code = 2030
name = "Merchant balances"
type = "liability"
kind = "control"
prefix = "merch"
status = "active"
"#;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % bound
    }
}

#[derive(Clone, Copy)]
enum Workload {
    HotWallet,
    Random,
    HotMerchant,
}

impl Workload {
    fn name(self) -> &'static str {
        match self {
            Workload::HotWallet => "hot wallet",
            Workload::Random => "random",
            Workload::HotMerchant => "hot merchant",
        }
    }
}

struct Scheme {
    name: &'static str,
    isolation: Isolation,
    lock_rows: bool,
}

const SCHEMES: [Scheme; 4] = [
    Scheme {
        name: "RC + locks",
        isolation: Isolation::ReadCommitted,
        lock_rows: true,
    },
    Scheme {
        name: "SER + locks",
        isolation: Isolation::Serializable,
        lock_rows: true,
    },
    Scheme {
        name: "SER, no locks",
        isolation: Isolation::Serializable,
        lock_rows: false,
    },
    Scheme {
        name: "RC, no locks",
        isolation: Isolation::ReadCommitted,
        lock_rows: false,
    },
];

#[derive(Default)]
struct Outcome {
    posted: usize,
    insufficient: usize,
    contention: usize,
    other: usize,
    other_example: Option<String>,
}

async fn run(admin: &PgConnectOptions, scheme: &Scheme, workload: Workload) -> Result<String> {
    // A fresh database for each run.
    let name = format!("mezgeb_contention_{}", std::process::id());
    let mut conn = PgConnection::connect_with(admin).await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
    )))
    .execute(&mut conn)
    .await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE {name} OWNER mezgeb_owner"
    )))
    .execute(&mut conn)
    .await?;
    let role = |user: &str| admin.clone().database(&name).username(user).password(user);
    let mut owner = PgConnection::connect_with(&role("mezgeb_owner")).await?;
    MIGRATOR.run(&mut owner).await?;
    owner.close().await?;

    let stats = Arc::new(RetryStats::default());
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(60))
        .connect_with(role("mezgeb_app"))
        .await?;
    let options = PostOptions {
        isolation: scheme.isolation,
        lock_rows: scheme.lock_rows,
        stats: Some(stats.clone()),
        ..PostOptions::default()
    };
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::with_options(pool.clone(), client, options);

    // Accounts: bank, 10 wallets, one merchant.
    let chart = Chart::from_toml(CHART)?;
    let bank: AccountId = uuid7(1).parse()?;
    let merchant: AccountId = uuid7(2).parse()?;
    store.open_account(bank, &chart, "1010").await?;
    store
        .open_account(merchant, &chart, "2030:merch-shop")
        .await?;
    let mut wallets = Vec::new();
    for n in 0..10u64 {
        let wallet: AccountId = uuid7(10 + n).parse()?;
        store
            .open_account(wallet, &chart, &format!("2010:cust-{n}"))
            .await?;
        let funding = match workload {
            Workload::HotWallet => 100,
            _ => 1_000_000,
        };
        store
            .post(
                uuid7(500_000 + n).parse()?,
                EntryDraft::default()
                    .debit(bank, etb(funding))
                    .credit(wallet, etb(funding)),
            )
            .await?;
        wallets.push(wallet);
    }
    let setup_attempts = stats.attempts();

    let mut rng = SplitMix(7);
    let mut drafts: Vec<(EntryId, EntryDraft)> = Vec::new();
    let count = match workload {
        Workload::HotWallet => 100,
        _ => 1_000,
    };
    for n in 0..count {
        let draft = match workload {
            Workload::HotWallet => EntryDraft::default()
                .debit(wallets[0], etb(80))
                .credit(merchant, etb(80)),
            Workload::Random => {
                let from = wallets[rng.next(10) as usize];
                let to = wallets[rng.next(10) as usize];
                let amount = 1 + rng.next(3_000) as i64;
                EntryDraft::default()
                    .debit(from, etb(amount))
                    .credit(to, etb(amount))
            }
            Workload::HotMerchant => {
                let from = wallets[rng.next(10) as usize];
                EntryDraft::default()
                    .debit(from, etb(100))
                    .credit(merchant, etb(100))
            }
        };
        drafts.push((uuid7(1_000_000 + n).parse()?, draft));
    }

    let started = Instant::now();
    let tasks: Vec<_> = drafts
        .into_iter()
        .map(|(id, draft)| {
            let mut store = store.clone();
            tokio::spawn(async move { store.post(id, draft).await })
        })
        .collect();
    let mut outcome = Outcome::default();
    for task in tasks {
        match task.await? {
            Ok(_) => outcome.posted += 1,
            Err(LedgerError::Posting(PostingError::InsufficientFunds { .. })) => {
                outcome.insufficient += 1;
            }
            Err(LedgerError::Contention(_) | LedgerError::Busy) => outcome.contention += 1,
            Err(other) => {
                outcome.other += 1;
                outcome.other_example.get_or_insert(other.to_string());
            }
        }
    }
    let elapsed = started.elapsed();

    let mut check = pool.acquire().await?;
    let clean = integrity_report(&mut check).await?.is_clean();
    drop(check);
    pool.close().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE {name} WITH (FORCE)"
    )))
    .execute(&mut conn)
    .await?;

    let attempts = stats.attempts() - setup_attempts;
    let retries = stats.deadlock_retries() + stats.serialization_retries();
    let mut row = format!(
        "| {} | {} | {} | {} | {} | {} | {} | {:.1}% | {} | {} | {:.0} | {} |",
        workload.name(),
        scheme.name,
        outcome.posted,
        outcome.insufficient,
        outcome.contention,
        outcome.other,
        attempts,
        100.0 * retries as f64 / count as f64,
        stats.deadlock_retries(),
        stats.gave_up(),
        count as f64 / elapsed.as_secs_f64(),
        if clean { "yes" } else { "NO" },
    );
    if let Some(example) = outcome.other_example {
        row.push_str(&format!("\n\n  other error example: {example}\n"));
    }
    Ok(row)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<()> {
    let url = std::env::args()
        .nth(1)
        .ok_or("usage: contention <superuser-database-url>")?;
    let admin: PgConnectOptions = url.parse()?;
    println!(
        "| Workload | Scheme | Posted | Insufficient funds | Gave up (contention) | Other errors | Transactions | Retry rate | Deadlock retries | Gave up after 3 | Ops/s | Integrity clean |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|");
    for workload in [Workload::HotWallet, Workload::Random, Workload::HotMerchant] {
        for scheme in &SCHEMES {
            println!("{}", run(&admin, scheme, workload).await?);
        }
    }
    Ok(())
}

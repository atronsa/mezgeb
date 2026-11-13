//! Baseline benchmark (step 1.16): postings per second and latency, through
//! the engine, on PostgreSQL.
//!
//! ```sh
//! cargo build --release -p mezgeb-bench
//! target/release/mezgeb-bench \
//!     --database-url postgres://postgres:postgres@localhost:55432/postgres
//! ```
//!
//! **Scenarios.** Entries of 2 or 4 lines, between accounts chosen at random
//! from `--wallets` wallets (*uniform*), or with every entry crediting one
//! merchant account (*hot*). A hot account serializes every posting on its
//! row lock, so its numbers are the ceiling for a single busy account.
//!
//! **Each run** gets a fresh database: migrations, a bank, the wallets (each
//! funded far beyond what the run can spend) and the hot account. Then
//! `--workers` workers, each on its own connection, post for `--warmup`
//! seconds, which are not counted, and `--seconds` more, which are. Every
//! posting must succeed, and a full verification must be clean at the end.
//! Each scenario runs `--runs` times, interleaved with the others, and the
//! spread of their throughput, `(max - min) / mean`, must stay under 10%.
//!
//! **Closed loop** (the default): each worker posts again as soon as its last
//! posting returns. That measures the most the engine can do with this many
//! connections. Its latencies are those of a saturated system, and they
//! understate what a client arriving at its own pace would see while the
//! system is backed up (coordinated omission).
//!
//! **Open loop** (`--rate N`): postings are due at a fixed N per second in
//! total, whether or not earlier ones have returned, and each latency is
//! measured from when the posting was due, not when it was sent. A backlog
//! then shows up in the latencies, as a client would feel it.
//!
//! **`--statements`** also prints the statements that took the most time,
//! from `pg_stat_statements`. The server must load it
//! (`shared_preload_libraries`); the local compose file does.
//!
//! The URL is a superuser's, used to create and drop the databases. The
//! roles from `db/init` must exist, with passwords equal to their names.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, IdempotencyKey, Ledger, Money,
};
use mezgeb_engine::{
    MIGRATOR, PgStore, PostOptions, PostRequest, RetryStats, Scope, VerifyOptions, post_entry_with,
    verify,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;

const CHART: &str = r#"
name = "Benchmark chart"
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
code = 2020
name = "Merchants"
type = "liability"
kind = "control"
prefix = "mer"
status = "active"
"#;

/// Each wallet's opening deposit, in santim: far more than a run can spend.
const DEPOSIT: i64 = 1_000_000_000_000;

/// Connections used to open and fund the accounts.
const SETUP_TASKS: u64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scenario {
    lines: u8,
    hot: bool,
}

impl Scenario {
    const ALL: [Scenario; 4] = [
        Scenario {
            lines: 2,
            hot: false,
        },
        Scenario {
            lines: 4,
            hot: false,
        },
        Scenario {
            lines: 2,
            hot: true,
        },
        Scenario {
            lines: 4,
            hot: true,
        },
    ];

    fn parse(name: &str) -> Result<Scenario> {
        Scenario::ALL
            .into_iter()
            .find(|s| s.name() == name)
            .ok_or_else(|| {
                format!("unknown scenario {name}; use 2-uniform, 4-uniform, 2-hot or 4-hot").into()
            })
    }

    fn name(&self) -> String {
        let accounts = if self.hot { "hot" } else { "uniform" };
        format!("{}-{accounts}", self.lines)
    }

    fn describe(&self) -> String {
        let accounts = if self.hot {
            "one hot account"
        } else {
            "uniform accounts"
        };
        format!("{}-line, {accounts}", self.lines)
    }
}

#[derive(Debug, Clone)]
struct Config {
    database_url: String,
    workers: u64,
    warmup: u64,
    seconds: u64,
    runs: u64,
    wallets: u64,
    rate: Option<u64>,
    scenarios: Vec<Scenario>,
    statements: bool,
}

fn parse(args: &[String]) -> Result<Config> {
    let mut config = Config {
        database_url: String::new(),
        workers: 16,
        warmup: 5,
        seconds: 20,
        runs: 3,
        wallets: 1_000,
        rate: None,
        scenarios: Scenario::ALL.to_vec(),
        statements: false,
    };
    let mut args = args.iter().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--statements" {
            config.statements = true;
            continue;
        }
        let value = args.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--database-url" => config.database_url = value.clone(),
            "--workers" => config.workers = value.parse()?,
            "--warmup" => config.warmup = value.parse()?,
            "--seconds" => config.seconds = value.parse()?,
            "--runs" => config.runs = value.parse()?,
            "--wallets" => config.wallets = value.parse()?,
            "--rate" => config.rate = Some(value.parse()?),
            "--scenarios" => {
                config.scenarios = value
                    .split(',')
                    .map(Scenario::parse)
                    .collect::<Result<_>>()?;
            }
            _ => return Err(format!("unknown option {flag}").into()),
        }
    }
    if config.database_url.is_empty() {
        return Err("--database-url is required".into());
    }
    if config.workers == 0 || config.seconds == 0 || config.runs == 0 {
        return Err("--workers, --seconds and --runs must be at least 1".into());
    }
    if config.wallets < 8 {
        return Err("--wallets must be at least 8".into());
    }
    if config.rate == Some(0) {
        return Err("--rate must be at least 1".into());
    }
    Ok(config)
}

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn client() -> Result<ClientId> {
    Ok(uuid7(0xbe4c).parse()?)
}

fn bank() -> Result<AccountId> {
    Ok(uuid7(1).parse()?)
}

fn hot() -> Result<AccountId> {
    Ok(uuid7(2).parse()?)
}

/// Wallet `n`, from 0.
fn wallet(n: u64) -> Result<AccountId> {
    Ok(uuid7(0x10_000 + n).parse()?)
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// SplitMix64: a small, fast random sequence per worker.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    /// Four different wallets.
    fn wallets(&mut self, count: u64) -> [u64; 4] {
        let mut chosen = [u64::MAX; 4];
        let mut i = 0;
        while i < chosen.len() {
            let w = self.below(count);
            if !chosen.contains(&w) {
                chosen[i] = w;
                i += 1;
            }
        }
        chosen
    }
}

/// One posting of the scenario. Amounts are small, so no wallet runs out.
fn draft(scenario: Scenario, rng: &mut Rng, wallets: u64) -> Result<EntryDraft> {
    let amount = 1 + i64::try_from(rng.below(100))?;
    let [a, b, c, d] = rng.wallets(wallets);
    let draft = EntryDraft::default();
    Ok(match (scenario.lines, scenario.hot) {
        (2, false) => draft
            .debit(wallet(a)?, etb(amount))
            .credit(wallet(b)?, etb(amount)),
        (2, true) => draft
            .debit(wallet(a)?, etb(amount))
            .credit(hot()?, etb(amount)),
        (_, false) => draft
            .debit(wallet(a)?, etb(amount))
            .debit(wallet(b)?, etb(amount))
            .credit(wallet(c)?, etb(amount))
            .credit(wallet(d)?, etb(amount)),
        (_, true) => draft
            .debit(wallet(a)?, etb(amount))
            .debit(wallet(b)?, etb(amount))
            .credit(hot()?, etb(amount))
            .credit(wallet(c)?, etb(amount)),
    })
}

fn request(key: String, draft: EntryDraft) -> Result<PostRequest> {
    Ok(PostRequest::new(
        EntryId::from_uuid(Uuid::now_v7())?,
        client()?,
        IdempotencyKey::new(&key)?,
        draft,
    ))
}

fn role(admin: &PgConnectOptions, database: &str, name: &str) -> PgConnectOptions {
    admin
        .clone()
        .database(database)
        .username(name)
        .password(name)
}

/// Opens the bank, the hot account and the wallets, and funds each wallet.
async fn setup(pool: &PgPool, wallets: u64) -> Result<()> {
    let chart = Arc::new(Chart::from_toml(CHART)?);
    let mut store = PgStore::new(pool.clone(), client()?);
    store.open_account(bank()?, &chart, "1010").await?;
    store.open_account(hot()?, &chart, "2020:mer-hot").await?;
    let mut tasks = Vec::new();
    for task in 0..SETUP_TASKS {
        let pool = pool.clone();
        let chart = Arc::clone(&chart);
        tasks.push(tokio::spawn(async move {
            // Opening an account takes a connection of its own, so open them
            // all before holding one for the deposits.
            let mut store = PgStore::new(pool.clone(), client()?);
            for n in (task..wallets).step_by(SETUP_TASKS as usize) {
                store
                    .open_account(wallet(n)?, &chart, &format!("2010:cust-{n}"))
                    .await?;
            }
            let mut conn = pool.acquire().await?;
            for n in (task..wallets).step_by(SETUP_TASKS as usize) {
                let deposit = EntryDraft::default()
                    .debit(bank()?, etb(DEPOSIT))
                    .credit(wallet(n)?, etb(DEPOSIT));
                post_entry_with(
                    &mut conn,
                    request(format!("deposit-{n}"), deposit)?,
                    &PostOptions::default(),
                )
                .await?;
            }
            Ok::<(), Error>(())
        }));
    }
    for task in tasks {
        task.await??;
    }
    Ok(())
}

/// When a worker starts posting, when counting starts and when it stops.
#[derive(Debug, Clone, Copy)]
struct Window {
    start: Instant,
    measure_from: Instant,
    end: Instant,
}

#[derive(Debug, Default)]
struct Sample {
    /// Latencies of the postings counted, in microseconds.
    latencies: Vec<u64>,
    errors: u64,
    first_error: Option<String>,
}

async fn worker(
    pool: PgPool,
    config: Arc<Config>,
    scenario: Scenario,
    id: u64,
    window: Window,
    stats: Arc<RetryStats>,
) -> Result<Sample> {
    let mut conn = pool.acquire().await?;
    let mut rng = Rng(id.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ 0x6d65_7a67_6562);
    let options = PostOptions {
        stats: Some(stats),
        ..PostOptions::default()
    };
    // Open loop: this worker's postings are due every `interval`, offset so
    // the workers' postings are spread evenly.
    let interval = config
        .rate
        .map(|rate| Duration::from_secs_f64(config.workers as f64 / rate as f64));
    let offset = interval
        .map(|i| i.mul_f64(id as f64 / config.workers as f64))
        .unwrap_or_default();
    let mut sample = Sample::default();
    let mut n: u32 = 0;
    loop {
        let due = match interval {
            Some(interval) => window.start + offset + interval * n,
            None => Instant::now(),
        };
        if due >= window.end {
            break;
        }
        if interval.is_some() {
            tokio::time::sleep_until(due.into()).await;
        }
        n += 1;
        let request = request(
            format!("w{id}-{n}"),
            draft(scenario, &mut rng, config.wallets)?,
        )?;
        let result = post_entry_with(&mut conn, request, &options).await;
        let latency = due.elapsed();
        match result {
            Ok(_) if due >= window.measure_from => {
                sample.latencies.push(u64::try_from(latency.as_micros())?);
            }
            Ok(_) => {}
            Err(error) => {
                sample.errors += 1;
                sample.first_error.get_or_insert_with(|| error.to_string());
            }
        }
    }
    Ok(sample)
}

#[derive(Debug, Clone)]
struct RunResult {
    postings: u64,
    per_second: f64,
    /// p50, p90, p99, p99.9 and max, in microseconds.
    latency: [u64; 5],
    errors: u64,
    deadlock_retries: u64,
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (p / 100.0 * (sorted.len() - 1) as f64).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn ms(micros: u64) -> String {
    format!("{:.1} ms", micros as f64 / 1_000.0)
}

async fn top_statements(conn: &mut PgConnection) -> Result<()> {
    let rows: Vec<(i64, f64, f64, String)> = sqlx::query_as(
        "SELECT calls, total_exec_time, mean_exec_time,
                left(regexp_replace(query, '\\s+', ' ', 'g'), 100)
         FROM pg_stat_statements
         WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
         ORDER BY total_exec_time DESC
         LIMIT 8",
    )
    .fetch_all(&mut *conn)
    .await?;
    println!("    statements by total time (measured window):");
    for (calls, total, mean, query) in rows {
        println!("      {total:>9.0} ms  {calls:>8} calls  {mean:>7.3} ms each  {query}");
    }
    Ok(())
}

/// One run of one scenario, in a database of its own.
async fn run_one(config: &Arc<Config>, scenario: Scenario) -> Result<RunResult> {
    let admin: PgConnectOptions = config.database_url.parse()?;
    let database = format!("mezgeb_bench_{}", std::process::id());
    let mut superuser = PgConnection::connect_with(&admin).await?;
    superuser
        .execute(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database} OWNER mezgeb_owner"
        )))
        .await?;
    let result = async {
        let mut owner =
            PgConnection::connect_with(&role(&admin, &database, "mezgeb_owner")).await?;
        MIGRATOR.run(&mut owner).await?;
        owner.close().await?;
        let pool = PgPoolOptions::new()
            .max_connections(u32::try_from(config.workers.max(SETUP_TASKS))?)
            .connect_with(role(&admin, &database, "mezgeb_app"))
            .await?;
        setup(&pool, config.wallets).await?;

        let mut statements = None;
        if config.statements {
            let mut conn = PgConnection::connect_with(&admin.clone().database(&database)).await?;
            match conn
                .execute("CREATE EXTENSION IF NOT EXISTS pg_stat_statements")
                .await
            {
                Ok(_) => statements = Some(conn),
                Err(error) => println!("    pg_stat_statements is not available: {error}"),
            }
        }

        let stats = Arc::new(RetryStats::default());
        let start = Instant::now();
        let measure_from = start + Duration::from_secs(config.warmup);
        let window = Window {
            start,
            measure_from,
            end: measure_from + Duration::from_secs(config.seconds),
        };
        let tasks: Vec<_> = (0..config.workers)
            .map(|id| {
                tokio::spawn(worker(
                    pool.clone(),
                    Arc::clone(config),
                    scenario,
                    id,
                    window,
                    Arc::clone(&stats),
                ))
            })
            .collect();
        if let Some(conn) = statements.as_mut() {
            tokio::time::sleep_until(measure_from.into()).await;
            conn.execute("SELECT pg_stat_statements_reset()").await?;
        }
        let mut latencies = Vec::new();
        let mut errors = 0;
        let mut first_error = None;
        for task in tasks {
            let sample = task.await??;
            latencies.extend(sample.latencies);
            errors += sample.errors;
            first_error = first_error.or(sample.first_error);
        }
        if let Some(error) = first_error {
            println!("    first error: {error}");
        }
        if let Some(mut conn) = statements {
            top_statements(&mut conn).await?;
            conn.close().await?;
        }
        pool.close().await;

        let mut verifier =
            PgConnection::connect_with(&role(&admin, &database, "mezgeb_verifier")).await?;
        let options = VerifyOptions {
            scope: Scope::Full,
            settle: Duration::ZERO,
            record: true,
        };
        let verification = verify(&mut verifier, &options).await?;
        verifier.close().await?;
        if !verification.is_clean() {
            return Err(format!("verification is not clean:\n{verification}").into());
        }

        latencies.sort_unstable();
        let postings = latencies.len() as u64;
        Ok(RunResult {
            postings,
            per_second: postings as f64 / config.seconds as f64,
            latency: [
                percentile(&latencies, 50.0),
                percentile(&latencies, 90.0),
                percentile(&latencies, 99.0),
                percentile(&latencies, 99.9),
                latencies.last().copied().unwrap_or_default(),
            ],
            errors,
            deadlock_retries: stats.deadlock_retries(),
        })
    }
    .await;
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

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<f64> = values.collect();
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

/// `(max - min) / mean` of the runs' throughput, in percent.
fn spread(runs: &[RunResult]) -> f64 {
    let rates = runs.iter().map(|r| r.per_second);
    let max = rates.clone().fold(f64::MIN, f64::max);
    let min = rates.clone().fold(f64::MAX, f64::min);
    let mean = mean(rates);
    if mean == 0.0 {
        return 0.0;
    }
    (max - min) / mean * 100.0
}

async fn run(config: Config) -> Result<bool> {
    let config = Arc::new(config);
    let mut admin = PgConnection::connect(&config.database_url).await?;
    let version: String = sqlx::query_scalar("SHOW server_version")
        .fetch_one(&mut admin)
        .await?;
    admin.close().await?;
    let mode = match config.rate {
        Some(rate) => format!("open loop at {rate} postings/s"),
        None => "closed loop".to_owned(),
    };
    println!(
        "PostgreSQL {version} · {} workers · {mode} · {} s warm-up + {} s measured · \
         {} wallets · {} runs per scenario",
        config.workers, config.warmup, config.seconds, config.wallets, config.runs
    );

    let mut results: Vec<Vec<RunResult>> = vec![Vec::new(); config.scenarios.len()];
    for run in 1..=config.runs {
        for (i, scenario) in config.scenarios.iter().enumerate() {
            let result = run_one(&config, *scenario).await?;
            let [p50, p90, p99, p999, max] = result.latency;
            println!(
                "{} run {run}/{}: {} postings, {:.0}/s · p50 {} · p90 {} · p99 {} · \
                 p99.9 {} · max {} · errors {} · deadlock retries {} · verify clean",
                scenario.describe(),
                config.runs,
                result.postings,
                result.per_second,
                ms(p50),
                ms(p90),
                ms(p99),
                ms(p999),
                ms(max),
                result.errors,
                result.deadlock_retries,
            );
            results[i].push(result);
        }
    }

    println!();
    println!(
        "| Scenario | Postings/s (mean of runs) | Runs | Spread | p50 | p99 | p99.9 | Errors |"
    );
    println!("|---|---:|---|---:|---:|---:|---:|---:|");
    let mut ok = true;
    for (scenario, runs) in config.scenarios.iter().zip(&results) {
        let spread = spread(runs);
        let errors: u64 = runs.iter().map(|r| r.errors).sum();
        let average = |i: usize| mean(runs.iter().map(|r| r.latency[i] as f64)) as u64;
        let each: Vec<String> = runs
            .iter()
            .map(|r| format!("{:.0}", r.per_second))
            .collect();
        println!(
            "| {} | {:.0} | {} | {spread:.1}% | {} | {} | {} | {errors} |",
            scenario.describe(),
            mean(runs.iter().map(|r| r.per_second)),
            each.join(", "),
            ms(average(0)),
            ms(average(2)),
            ms(average(3)),
        );
        if spread >= 10.0 || errors > 0 {
            ok = false;
        }
    }
    println!();
    if ok {
        println!("PASS: no errors, every verification clean, every spread under 10%");
    } else {
        println!("FAIL: errors, or a spread of 10% or more");
    }
    Ok(ok)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let config = match parse(&args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    match run(config).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &str) -> Vec<String> {
        std::iter::once("mezgeb-bench")
            .chain(list.split_whitespace())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn options_are_parsed() -> Result<()> {
        let config = parse(&args(
            "--database-url postgres://x --workers 4 --rate 500 --scenarios 2-hot,4-uniform --statements",
        ))?;
        assert_eq!(config.workers, 4);
        assert_eq!(config.rate, Some(500));
        assert!(config.statements);
        assert_eq!(config.scenarios.len(), 2);
        assert_eq!(config.scenarios[0].name(), "2-hot");
        assert!(parse(&args("--workers 4")).is_err());
        assert!(parse(&args("--database-url x --scenarios 3-hot")).is_err());
        assert!(parse(&args("--database-url x --wallets 4")).is_err());
        Ok(())
    }

    #[test]
    fn every_scenario_balances_and_uses_different_wallets() -> Result<()> {
        let mut rng = Rng(7);
        let hot = hot()?;
        for scenario in Scenario::ALL {
            for _ in 0..1_000 {
                let lines = draft(scenario, &mut rng, 8)?.lines;
                assert_eq!(lines.len(), usize::from(scenario.lines));
                let signed: i64 = lines
                    .iter()
                    .map(|l| match l.side {
                        mezgeb_core::Side::Debit => l.amount.amount(),
                        mezgeb_core::Side::Credit => -l.amount.amount(),
                    })
                    .sum();
                assert_eq!(signed, 0, "{} does not balance", scenario.name());
                let mut accounts: Vec<AccountId> = lines.iter().map(|l| l.account).collect();
                accounts.sort();
                accounts.dedup();
                assert_eq!(accounts.len(), lines.len());
                assert_eq!(
                    lines.iter().any(|l| l.account == hot),
                    scenario.hot,
                    "{}",
                    scenario.name()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn percentiles_pick_the_nearest_rank() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&sorted, 50.0), 51);
        assert_eq!(percentile(&sorted, 99.0), 99);
        assert_eq!(percentile(&sorted, 100.0), 100);
        assert_eq!(percentile(&[], 50.0), 0);
    }
}

//! Deterministic simulator, v1 (step 1.15, spec §10).
//!
//! Runs a random workload of deposits, transfers, adjustments, reversals and
//! holds (place, capture, release, expire, time passing) and checks the
//! ledger against its reference implementation at every step.
//!
//! ```sh
//! # Reference ledger only: fast, invariants checked as it goes.
//! cargo run --release -p mezgeb-sim -- --seed 42 --ops 1000000
//!
//! # Against PostgreSQL as well, with faults injected into the engine.
//! cargo run --release -p mezgeb-sim -- --seed 42 --ops 100000 --lanes 16 \
//!     --faults 100 --database-url postgres://postgres:postgres@localhost:55432/postgres
//! ```
//!
//! **Lanes.** The workload is split into `--lanes` independent ledgers, each
//! with its own accounts, client, clock and reference ledger
//! ([`workload::Lane`]). Lanes share nothing, so they run in parallel on one
//! database and each operation still has exactly one right answer.
//!
//! **Against PostgreSQL** (`--database-url`, a superuser's URL, used only
//! to create and drop a throwaway database), every operation runs through
//! the engine first. `--faults` per thousand attempts fail at a random
//! fault point; a failed attempt is retried with the same idempotency key
//! until one runs without a fault ([`pg::run`]). The answer must equal the
//! reference ledger's, entry for entry and line for line. At the end every
//! account and hold is compared, each lane has exactly one entry per
//! successful operation, and a full verification must be clean. The roles
//! from `db/init` must exist, with passwords equal to their names (the
//! local and CI setup).
//!
//! **Deterministic.** The same seed, ops, lanes and fault rate give the same
//! operations, the same faults and the same results, so a failure prints
//! what is needed to replay it.

mod pg;
mod rng;
mod workload;

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use mezgeb_core::{Holds, IdempotencyKey, Ledger};
use mezgeb_engine::{MIGRATOR, PgStore, Scope, VerifyOptions, verify};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, PgPool};

use crate::pg::Faults;
use crate::workload::{Error, Lane, Op, Outcome};

/// How often each lane runs the reference ledger's invariant checker and
/// compares its accounts with the database.
const CHECK_EVERY: u64 = 2_000;

#[derive(Debug, Clone)]
struct Config {
    seed: u64,
    ops: u64,
    lanes: u64,
    faults: u64,
    database_url: Option<String>,
}

fn parse(args: &[String]) -> Result<Config, Error> {
    let mut config = Config {
        seed: 1,
        ops: 10_000,
        lanes: 8,
        faults: 100,
        database_url: None,
    };
    let mut args = args.iter().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--seed" => config.seed = value.parse()?,
            "--ops" => config.ops = value.parse()?,
            "--lanes" => config.lanes = value.parse()?,
            "--faults" => config.faults = value.parse()?,
            "--database-url" => config.database_url = Some(value.clone()),
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    if config.lanes == 0 || config.lanes > 255 || config.faults > 1_000 {
        return Err("--lanes must be 1 to 255 and --faults 0 to 1000".into());
    }
    Ok(config)
}

/// What one lane did.
#[derive(Debug, Default)]
struct Report {
    ops: BTreeMap<&'static str, u64>,
    written: u64,
    refused: u64,
    faults: Faults,
    checks: u64,
}

impl Report {
    fn add(&mut self, other: &Report) {
        for (kind, n) in &other.ops {
            *self.ops.entry(kind).or_default() += n;
        }
        self.written += other.written;
        self.refused += other.refused;
        self.faults.add(&other.faults);
        self.checks += other.checks;
    }

    fn count(&mut self, op: &Op, outcome: &Outcome) {
        *self.ops.entry(op.kind()).or_default() += 1;
        match outcome {
            Outcome::Refused(_) => self.refused += 1,
            Outcome::Nothing => {}
            _ => self.written += 1,
        }
    }
}

/// The ops for lane `n` of `lanes`: an even share.
fn share(config: &Config, n: u64) -> u64 {
    config.ops / config.lanes + u64::from(n < config.ops % config.lanes)
}

/// The reference ledger alone.
fn run_reference(config: &Config) -> Result<Report, Error> {
    let mut total = Report::default();
    for n in 0..config.lanes {
        let mut lane = Lane::new(n, config.seed)?;
        let mut report = Report::default();
        for step in 0..share(config, n) {
            let (op, held) = lane.next_op()?;
            if let Some((id, reference)) = held {
                lane.open_held(id, &reference)?;
            }
            let entry = lane.fresh_entry()?;
            let outcome = lane.apply(&op, entry)?;
            report.count(&op, &outcome);
            if (step + 1) % CHECK_EVERY == 0 {
                lane.check()?;
                report.checks += 1;
            }
        }
        lane.check()?;
        report.checks += 1;
        total.add(&report);
    }
    Ok(total)
}

/// One lane against PostgreSQL and its reference ledger.
async fn run_lane(
    mut lane: Lane,
    ops: u64,
    faults: u64,
    pool: PgPool,
) -> Result<(Lane, Report), Error> {
    let mut store = PgStore::new(pool.clone(), lane.client);
    for (id, reference) in lane.accounts()? {
        store.open_account(id, &lane.chart, &reference).await?;
    }
    let mut conn = pool.acquire().await?;
    let mut report = Report::default();
    for step in 0..ops {
        let (op, held) = lane.next_op()?;
        if let Some((id, reference)) = held {
            lane.open_held(id, &reference)?;
            store.open_account(id, &lane.chart, &reference).await?;
        }
        let key = IdempotencyKey::new(format!("l{}-{step}", lane.number))?;
        let actual = pg::run(&mut conn, &mut lane, &op, &key, faults, &mut report.faults).await?;
        // The reference ledger writes the entry the database wrote.
        let entry = match actual.entry(&op) {
            Some(id) => id,
            None => lane.fresh_entry()?,
        };
        let expected = lane.apply(&op, entry)?;
        if actual != expected {
            return Err(format!(
                "lane {} step {step}: {op:?}\n  PostgreSQL: {actual:?}\n  reference:  {expected:?}",
                lane.number
            )
            .into());
        }
        report.count(&op, &actual);
        if (step + 1) % CHECK_EVERY == 0 {
            lane.check()?;
            for (id, _) in lane.accounts()? {
                let stored = store.get_account(id).await?;
                if stored.as_ref() != lane.oracle.account(id) {
                    return Err(
                        format!("lane {} step {step}: account {id} differs", lane.number).into(),
                    );
                }
            }
            report.checks += 1;
        }
    }
    lane.check()?;
    report.checks += 1;
    Ok((lane, report))
}

/// A connection option set for one of the ledger's roles.
fn role(admin: &PgConnectOptions, database: &str, name: &str) -> PgConnectOptions {
    admin
        .clone()
        .database(database)
        .username(name)
        .password(name)
}

async fn run_database(config: &Config, url: &str) -> Result<Report, Error> {
    let admin: PgConnectOptions = url.parse()?;
    let database = format!("mezgeb_sim_{}_{}", config.seed, std::process::id());
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
            .max_connections(u32::try_from(config.lanes * 2)?)
            .acquire_timeout(Duration::from_secs(600))
            .connect_with(role(&admin, &database, "mezgeb_app"))
            .await?;

        let mut tasks = Vec::new();
        for n in 0..config.lanes {
            let lane = Lane::new(n, config.seed)?;
            tasks.push(tokio::spawn(run_lane(
                lane,
                share(config, n),
                config.faults,
                pool.clone(),
            )));
        }
        let mut lanes = Vec::new();
        let mut total = Report::default();
        for task in tasks {
            let (lane, report) = task.await??;
            total.add(&report);
            lanes.push(lane);
        }

        // Every account and hold, and each lane's entry count, match.
        let mut store = PgStore::new(pool.clone(), lanes[0].client);
        let accounts = store.all_accounts().await?;
        let holds = store.all_holds().await?;
        let mut expected_accounts = Vec::new();
        let mut expected_holds = Vec::new();
        for lane in &lanes {
            expected_accounts.extend(lane.oracle.accounts().cloned());
            expected_holds.extend(lane.oracle.holds().cloned());
            let entries: i64 =
                sqlx::query_scalar("SELECT count(*) FROM journal_entries WHERE client_id = $1")
                    .bind(lane.client.as_uuid())
                    .fetch_one(&pool)
                    .await?;
            if u64::try_from(entries)? != lane.oracle.entries().len() as u64 {
                return Err(format!(
                    "lane {}: {entries} entries in PostgreSQL, {} in the reference ledger",
                    lane.number,
                    lane.oracle.entries().len()
                )
                .into());
            }
        }
        expected_accounts.sort_by_key(|a| a.id());
        expected_holds.sort_by_key(|h| h.id());
        if accounts != expected_accounts {
            return Err("the accounts differ at the end".into());
        }
        if holds != expected_holds {
            return Err("the holds differ at the end".into());
        }
        println!(
            "compared {} accounts, {} holds ({} still active) and {} entries: identical",
            accounts.len(),
            holds.len(),
            lanes.iter().map(Lane::active_holds).sum::<usize>(),
            lanes
                .iter()
                .map(|l| l.oracle.entries().len())
                .sum::<usize>()
        );
        pool.close().await;

        let mut verifier =
            PgConnection::connect_with(&role(&admin, &database, "mezgeb_verifier")).await?;
        let options = VerifyOptions {
            scope: Scope::Full,
            settle: Duration::ZERO,
            record: true,
        };
        let verification = verify(&mut verifier, &options).await?;
        println!("{verification}");
        verifier.close().await?;
        if !verification.is_clean() {
            return Err("verification is not clean".into());
        }
        Ok(total)
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

fn print(config: &Config, report: &Report, elapsed: Duration) {
    let ops: u64 = report.ops.values().sum();
    println!(
        "seed {} · {ops} operations in {} lanes · {:.1} s ({:.0} ops/s)",
        config.seed,
        config.lanes,
        elapsed.as_secs_f64(),
        ops as f64 / elapsed.as_secs_f64().max(0.001)
    );
    let kinds: Vec<String> = Op::KINDS
        .iter()
        .map(|k| format!("{k} {}", report.ops.get(k).copied().unwrap_or_default()))
        .collect();
    println!("  operations: {}", kinds.join(", "));
    println!(
        "  results: {} written, {} refused by the rules; invariants checked {} times",
        report.written, report.refused, report.checks
    );
    if config.database_url.is_some() {
        let [lock, lines, commit, after] = report.faults.fired;
        println!(
            "  faults injected: {} (before lock {lock}, after lines {lines}, before commit {commit}, \
             after commit {after}), each retried to the same result as the reference ledger",
            report.faults.total()
        );
    }
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
    let started = Instant::now();
    let result = match &config.database_url {
        None => run_reference(&config),
        Some(url) => run_database(&config, url).await,
    };
    match result {
        Ok(report) => {
            print(&config, &report, started.elapsed());
            println!("PASS");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "FAIL (seed {}, ops {}, lanes {}, faults {}): {error}",
                config.seed, config.ops, config.lanes, config.faults
            );
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same seed gives the same run.
    #[test]
    fn the_reference_run_is_deterministic() -> Result<(), Error> {
        let config = Config {
            seed: 7,
            ops: 5_000,
            lanes: 3,
            faults: 0,
            database_url: None,
        };
        let first = run_reference(&config)?;
        let second = run_reference(&config)?;
        assert_eq!(first.ops, second.ops);
        assert_eq!(
            (first.written, first.refused),
            (second.written, second.refused)
        );
        assert!(first.written > 0 && first.refused > 0);
        Ok(())
    }

    #[test]
    fn options_are_parsed() -> Result<(), Error> {
        let args: Vec<String> = ["sim", "--seed", "3", "--ops", "10", "--lanes", "2"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let config = parse(&args)?;
        assert_eq!((config.seed, config.ops, config.lanes), (3, 10, 2));
        assert!(parse(&["sim".to_owned(), "--lanes".to_owned(), "0".to_owned()]).is_err());
        Ok(())
    }
}

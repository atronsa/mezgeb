//! Chaos load for step 1.15: transfers between wallets while PostgreSQL is
//! killed, and a check afterwards that every request the client saw
//! succeed exists, and every other one exists once or not at all.
//!
//! `scripts/chaos.sh` drives it: `load` runs while the database is killed
//! part-way, `check` runs once it is back.
//!
//! ```sh
//! chaos_load setup <owner-url> <app-url>
//! chaos_load load  <app-url> <log> <cycle> <workers> <seconds>
//! chaos_load check <app-url> <log> <cycle|all>
//! ```
//!
//! **The log is the client's memory.** Before sending a request, a worker
//! writes `sent <key> <from> <to> <amount>`; when the answer arrives it
//! writes `ack <key> <entry>`, `rejected <key> <code>` or
//! `failed <key> <error>`. A request killed in flight has `failed` and an
//! unknown outcome: it may have committed before the database died.
//!
//! **`check`** compares the log with the journal:
//!
//! - every acknowledged request has exactly one entry, the one acknowledged;
//! - every rejected request has none;
//! - every request with an unknown outcome has one or none.
//!
//! Then it resolves each unknown outcome the way a client would, by
//! retrying the same request with the same key. One that had committed must
//! return its existing entry; one that had not is posted (or rejected) now.
//! Afterwards every request has had its effect exactly once, and the money
//! in the wallets adds up to what was deposited. Exit status 1 on any
//! violation.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, IdempotencyKey, Ledger, LedgerError,
    Money,
};
use mezgeb_engine::{MIGRATOR, PgStore, PostOptions, PostRequest, post_entry_with};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

const CHART: &str = r#"
name = "Chaos chart"
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
"#;

const WALLETS: u64 = 10;
/// Each wallet's deposit, in santim.
const DEPOSIT: i64 = 1_000_000;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn bank() -> Result<AccountId> {
    Ok(uuid7(1).parse()?)
}

/// Wallet `n`, from 0.
fn wallet(n: u64) -> Result<AccountId> {
    Ok(uuid7(2 + n).parse()?)
}

fn client() -> Result<ClientId> {
    Ok(uuid7(0xc4a05).parse()?)
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn transfer(from: u64, to: u64, amount: i64) -> Result<EntryDraft> {
    Ok(EntryDraft::default()
        .debit(wallet(from)?, etb(amount))
        .credit(wallet(to)?, etb(amount)))
}

fn request(key: &str, draft: EntryDraft) -> Result<PostRequest> {
    Ok(PostRequest::new(
        EntryId::from_uuid(Uuid::now_v7())?,
        client()?,
        IdempotencyKey::new(key)?,
        draft,
    ))
}

/// Migrates, opens the bank and the wallets, and funds each wallet once.
async fn setup(owner_url: &str, app_url: &str) -> Result<()> {
    let mut owner = PgConnection::connect(owner_url).await?;
    MIGRATOR.run(&mut owner).await?;
    owner.close().await?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(app_url)
        .await?;
    let mut store = PgStore::new(pool.clone(), client()?);
    let chart = Chart::from_toml(CHART)?;
    if store.get_account(bank()?).await?.is_none() {
        store.open_account(bank()?, &chart, "1010").await?;
    }
    for n in 0..WALLETS {
        if store.get_account(wallet(n)?).await?.is_none() {
            store
                .open_account(wallet(n)?, &chart, &format!("2010:cust-{n}"))
                .await?;
        }
    }
    let mut conn = pool.acquire().await?;
    for n in 0..WALLETS {
        let deposit = EntryDraft::default()
            .debit(bank()?, etb(DEPOSIT))
            .credit(wallet(n)?, etb(DEPOSIT));
        post_entry_with(
            &mut conn,
            request(&format!("deposit-{n}"), deposit)?,
            &PostOptions::default(),
        )
        .await?;
    }
    println!("ready: bank and {WALLETS} wallets of {DEPOSIT} each");
    Ok(())
}

/// The client's log, one line per event, flushed at once.
#[derive(Clone)]
struct Log(Arc<Mutex<BufWriter<File>>>);

impl Log {
    fn open(path: &str) -> Result<Log> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Log(Arc::new(Mutex::new(BufWriter::new(file)))))
    }

    fn write(&self, line: &str) -> Result<()> {
        let mut file = self.0.lock().map_err(|_| "log lock poisoned")?;
        writeln!(file, "{line}")?;
        file.flush()?;
        Ok(())
    }
}

/// A small deterministic random sequence per worker.
struct Rng(u64);

impl Rng {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

#[derive(Debug, Default)]
struct Counts {
    sent: u64,
    acked: u64,
    rejected: u64,
    failed: u64,
}

async fn worker(pool: PgPool, log: Log, cycle: u64, id: u64, deadline: Instant) -> Result<Counts> {
    let mut rng = Rng(cycle * 1_000 + id);
    let mut counts = Counts::default();
    let mut n = 0u64;
    while Instant::now() < deadline {
        n += 1;
        let key = format!("c{cycle}-w{id}-{n}");
        let from = rng.next(WALLETS);
        let to = (from + 1 + rng.next(WALLETS - 1)) % WALLETS;
        // One in 20 asks for more than a wallet can hold, to be rejected.
        let amount = if rng.next(20) == 0 {
            WALLETS as i64 * DEPOSIT + 1
        } else {
            1 + rng.next(5_000) as i64
        };
        log.write(&format!("sent {key} {from} {to} {amount}"))?;
        counts.sent += 1;
        let outcome = match pool.acquire().await {
            Ok(mut conn) => {
                let request = request(&key, transfer(from, to, amount)?)?;
                post_entry_with(&mut conn, request, &PostOptions::default()).await
            }
            Err(e) => Err(LedgerError::Storage(e.to_string())),
        };
        match outcome {
            Ok(entry) => {
                log.write(&format!("ack {key} {}", entry.id()))?;
                counts.acked += 1;
            }
            Err(LedgerError::Posting(e)) => {
                log.write(&format!("rejected {key} {}", e.code()))?;
                counts.rejected += 1;
            }
            Err(e) => {
                let error = e.to_string().replace('\n', " ");
                log.write(&format!("failed {key} {error}"))?;
                counts.failed += 1;
                // The database is down: do not spin.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    Ok(counts)
}

/// Sends transfers from `workers` workers for `seconds` seconds, whatever
/// happens to the database meanwhile.
async fn load(url: &str, path: &str, cycle: u64, workers: u64, seconds: u64) -> Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(u32::try_from(workers)?)
        .acquire_timeout(Duration::from_secs(1))
        .connect_lazy(url)?;
    let log = Log::open(path)?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let tasks: Vec<_> = (0..workers)
        .map(|id| tokio::spawn(worker(pool.clone(), log.clone(), cycle, id, deadline)))
        .collect();
    let mut total = Counts::default();
    for task in tasks {
        let counts = task.await??;
        total.sent += counts.sent;
        total.acked += counts.acked;
        total.rejected += counts.rejected;
        total.failed += counts.failed;
    }
    println!(
        "LOAD cycle={cycle} sent={} acked={} rejected={} failed={}",
        total.sent, total.acked, total.rejected, total.failed
    );
    Ok(())
}

/// What the client knows about one request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Known {
    /// Sent; the answer was an error or never came.
    Unknown,
    Acked(Uuid),
    Rejected,
}

#[derive(Debug, Clone)]
struct Sent {
    from: u64,
    to: u64,
    amount: i64,
    known: Known,
}

/// Reads the log: every request with the given key prefix, and what the
/// client knows of it.
fn read_log(path: &str, prefix: &str) -> Result<BTreeMap<String, Sent>> {
    let mut requests: BTreeMap<String, Sent> = BTreeMap::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        let words: Vec<&str> = line.split(' ').collect();
        let (Some(event), Some(key)) = (words.first(), words.get(1)) else {
            continue;
        };
        if !key.starts_with(prefix) {
            continue;
        }
        let key = (*key).to_owned();
        match *event {
            "sent" => {
                let [_, _, from, to, amount] = words[..] else {
                    return Err(format!("bad log line: {line}").into());
                };
                requests.insert(
                    key,
                    Sent {
                        from: from.parse()?,
                        to: to.parse()?,
                        amount: amount.parse()?,
                        known: Known::Unknown,
                    },
                );
            }
            "ack" | "retry-ack" => {
                let id = words.get(2).ok_or("ack without entry")?.parse()?;
                requests.get_mut(&key).ok_or("ack before sent")?.known = Known::Acked(id);
            }
            "rejected" | "retry-rejected" => {
                requests.get_mut(&key).ok_or("rejected before sent")?.known = Known::Rejected;
            }
            // An error: the outcome stays unknown.
            "failed" => {}
            _ => return Err(format!("bad log line: {line}").into()),
        }
    }
    Ok(requests)
}

/// The entries in the journal for keys with this prefix: key → entry IDs.
async fn journal(pool: &PgPool, prefix: &str) -> Result<BTreeMap<String, Vec<Uuid>>> {
    let rows: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT idempotency_key, id FROM journal_entries
         WHERE client_id = $1 AND starts_with(idempotency_key, $2)",
    )
    .bind(client()?.as_uuid())
    .bind(prefix)
    .fetch_all(pool)
    .await?;
    let mut entries: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
    for (key, id) in rows {
        entries.entry(key).or_default().push(id);
    }
    Ok(entries)
}

/// Checks one cycle's requests (or all) against the journal, resolves the
/// unknown ones by retrying them, and checks again.
async fn check(url: &str, path: &str, cycle: &str) -> Result<bool> {
    let prefix = if cycle == "all" {
        "c".to_owned()
    } else {
        format!("c{}-", cycle.parse::<u64>()?)
    };
    let pool = PgPoolOptions::new().max_connections(2).connect(url).await?;
    let log = Log::open(path)?;
    let requests = read_log(path, &prefix)?;
    let entries = journal(&pool, &prefix).await?;
    let mut violations = Vec::new();
    let (mut acked, mut rejected, mut committed, mut absent) = (0, 0, 0, 0);
    let mut unknown = Vec::new();
    for (key, sent) in &requests {
        let found = entries.get(key).map(Vec::as_slice).unwrap_or_default();
        match (&sent.known, found) {
            (Known::Acked(id), [entry]) if entry == id => acked += 1,
            (Known::Acked(id), _) => violations.push(format!(
                "{key}: acknowledged as {id}, the journal has {found:?}"
            )),
            (Known::Rejected, []) => rejected += 1,
            (Known::Rejected, _) => {
                violations.push(format!("{key}: rejected, the journal has {found:?}"));
            }
            (Known::Unknown, []) => {
                absent += 1;
                unknown.push((key, sent, None));
            }
            (Known::Unknown, [entry]) => {
                committed += 1;
                unknown.push((key, sent, Some(*entry)));
            }
            (Known::Unknown, _) => {
                violations.push(format!("{key}: unknown outcome, the journal has {found:?}"));
            }
        }
    }
    let stray = entries
        .keys()
        .filter(|k| !requests.contains_key(*k))
        .count();
    if stray > 0 {
        violations.push(format!("{stray} entries for keys the client never sent"));
    }

    // Resolve each unknown outcome as a client would: retry it.
    let mut conn = pool.acquire().await?;
    for (key, sent, existing) in &unknown {
        let retry = request(key, transfer(sent.from, sent.to, sent.amount)?)?;
        match (
            post_entry_with(&mut conn, retry, &PostOptions::default()).await,
            existing,
        ) {
            (Ok(entry), Some(id)) if entry.id().as_uuid() == *id => {
                log.write(&format!("retry-ack {key} {id}"))?;
            }
            (Ok(entry), Some(id)) => violations.push(format!(
                "{key}: committed as {id}, but the retry returned {}",
                entry.id()
            )),
            (Ok(entry), None) => log.write(&format!("retry-ack {key} {}", entry.id()))?,
            (Err(LedgerError::Posting(e)), None) => {
                log.write(&format!("retry-rejected {key} {}", e.code()))?;
            }
            (Err(e), _) => violations.push(format!("{key}: the retry failed: {e}")),
        }
    }
    drop(conn);

    // Now every request has had its effect exactly once.
    let entries = journal(&pool, &prefix).await?;
    for (key, sent) in read_log(path, &prefix)? {
        let found = entries.get(&key).map(Vec::len).unwrap_or_default();
        let expected = match sent.known {
            Known::Acked(_) => 1,
            Known::Rejected => 0,
            Known::Unknown => {
                violations.push(format!("{key}: still unknown after the retry"));
                continue;
            }
        };
        if found != expected {
            violations.push(format!(
                "{key}: {found} entries after the retry, {expected} expected"
            ));
        }
    }
    // Transfers move money between wallets: the total never changes.
    let total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(credits_total - debits_total), 0)::bigint
         FROM accounts WHERE code = '2010'",
    )
    .fetch_one(&pool)
    .await?;
    if total != WALLETS as i64 * DEPOSIT {
        violations.push(format!(
            "the wallets hold {total}, not {}",
            WALLETS as i64 * DEPOSIT
        ));
    }
    for violation in &violations {
        println!("VIOLATION {violation}");
    }
    println!(
        "CHECK cycle={cycle} requests={} acked={acked} rejected={rejected} unknown={} \
         unknown_committed={committed} unknown_absent={absent} violations={}",
        requests.len(),
        unknown.len(),
        violations.len()
    );
    Ok(violations.is_empty())
}

async fn run(args: &[String]) -> Result<bool> {
    let arg = |i: usize| -> Result<&str> {
        args.get(i)
            .map(String::as_str)
            .ok_or_else(|| "missing argument; see the usage in chaos_load.rs".into())
    };
    match arg(1)? {
        "setup" => setup(arg(2)?, arg(3)?).await.map(|()| true),
        "load" => load(
            arg(2)?,
            arg(3)?,
            arg(4)?.parse()?,
            arg(5)?.parse()?,
            arg(6)?.parse()?,
        )
        .await
        .map(|()| true),
        "check" => check(arg(2)?, arg(3)?, arg(4)?).await,
        other => Err(format!("unknown command {other}").into()),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

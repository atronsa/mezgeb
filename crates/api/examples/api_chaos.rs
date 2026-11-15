//! The step 1.15 chaos run, through the API (step 2.07 exit gate):
//! transfers sent over HTTP while PostgreSQL or the service itself is
//! killed, and a check afterwards that every request the client saw succeed
//! exists, and every other one exists once or not at all.
//!
//! `scripts/chaos-api.sh` drives it: `load` runs while something is killed
//! part-way, `check` runs once everything is back.
//!
//! ```sh
//! api_chaos setup <owner-url> <base-url>
//! api_chaos load  <base-url> <log> <cycle> <workers> <seconds>
//! api_chaos check <app-url> <base-url> <log> <cycle|all>
//! ```
//!
//! It logs in with the development credentials of `deploy/clients.dev.toml`
//! (`POST /oauth/token`) when each command starts; a command runs for less
//! than a token lives.
//!
//! **The log is the client's memory**, as in the engine's `chaos_load`:
//! `sent <key> <from> <to> <amount>` before each request, then
//! `ack <key> <entry>` (201), `rejected <key> <code>` (422) or
//! `failed <key> <reason>` (no answer, or 5xx: the outcome is unknown). Any
//! other answer (400, 401, 409...) is `odd`: a well-formed request should
//! never get one, so the check counts it as a violation.
//!
//! **`check`** compares the log with the journal, then resolves each
//! unknown outcome the way a client would, by retrying it with the same key
//! through the API: one that had committed must come back as a replay of
//! its entry; one that had not is posted (or refused) now. Afterwards every
//! request has had its effect exactly once and the wallets still hold what
//! was deposited. Exit status 1 on any violation.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mezgeb_engine::MIGRATOR;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

/// The wallet credential (`poster`, `reader`) and the operator's (`admin`
/// among others), with their development secrets.
const CLIENT: (&str, &str) = (
    "0192f3c4-0000-7000-8000-0000000c0001",
    "dev-wallet-secret-0001",
);
const OPERATOR: (&str, &str) = (
    "0192f3c4-0000-7000-8000-0000000c0002",
    "dev-operator-secret-0002",
);
/// The client the wallet credential belongs to.
const CLIENT_ID: &str = "0192f3c4-0000-7000-8000-00000000c11e";

const WALLETS: u64 = 10;
/// Each wallet's deposit, in santim.
const DEPOSIT: i64 = 1_000_000;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn wallet(n: u64) -> String {
    format!("2010:cust-{n}")
}

/// Santim as an API amount: `1234` → `"12.34"`.
fn etb(santim: i64) -> Value {
    json!({"amount": format!("{}.{:02}", santim / 100, santim % 100), "currency": "ETB"})
}

fn transfer(from: u64, to: u64, amount: i64) -> Value {
    json!({"accounts": {"payer_wallet": wallet(from), "payee_wallet": wallet(to)},
           "amounts": {"amount": etb(amount)}})
}

/// An access token for a credential.
async fn token(http: &reqwest::Client, base: &str, (id, secret): (&str, &str)) -> Result<String> {
    let response = http
        .post(format!("{base}/oauth/token"))
        .basic_auth(id, Some(secret))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await?;
    let status = response.status().as_u16();
    let body: Value = response.json().await?;
    body["access_token"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("no token for {id}: {status} {body}").into())
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?)
}

/// What the API answered.
enum Answer {
    /// 201, or 200 for a replay: the entry.
    Posted { entry: String, replayed: bool },
    /// 422: refused by a rule.
    Refused(String),
    /// No answer, or 5xx: the outcome is unknown.
    Unknown(String),
    /// Anything else: should not happen.
    Odd(u16, String),
}

async fn post(
    http: &reqwest::Client,
    base: &str,
    path: &str,
    token: &str,
    key: &str,
    body: &Value,
) -> Answer {
    let sent = http
        .post(format!("{base}{path}"))
        .bearer_auth(token)
        .header("Idempotency-Key", key)
        .json(body)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(e) => return Answer::Unknown(format!("no answer: {e}").replace(['\n', ' '], "_")),
    };
    let status = response.status().as_u16();
    let replayed = response.headers().contains_key("idempotent-replayed");
    let body: Value = match response.json().await {
        Ok(body) => body,
        Err(e) => {
            return Answer::Unknown(format!("{status}: unreadable body: {e}").replace(' ', "_"));
        }
    };
    let code = body["code"].as_str().unwrap_or("none").to_owned();
    match status {
        200 | 201 => match body["id"].as_str() {
            Some(id) => Answer::Posted {
                entry: id.to_owned(),
                replayed,
            },
            None => Answer::Odd(status, "no id".into()),
        },
        422 => Answer::Refused(code),
        500..=599 => Answer::Unknown(format!("{status}:{code}")),
        _ => Answer::Odd(status, code),
    }
}

/// Migrates, then opens the bank and the wallets and funds each wallet
/// once, all through the API.
async fn setup(owner_url: &str, base: &str) -> Result<()> {
    let mut owner = PgConnection::connect(owner_url).await?;
    MIGRATOR.run(&mut owner).await?;
    owner.close().await?;
    let http = http()?;
    let client = token(&http, base, CLIENT).await?;
    let operator = token(&http, base, OPERATOR).await?;
    let open = |token: String, reference: String| {
        let http = http.clone();
        async move {
            let response = http
                .post(format!("{base}/v1/accounts"))
                .bearer_auth(token)
                .header("Idempotency-Key", format!("open-{reference}"))
                .json(&json!({"reference": reference}))
                .send()
                .await?;
            match response.status().as_u16() {
                200 | 201 => Result::Ok(()),
                status => {
                    let text = response.text().await?;
                    Err(format!("opening {reference}: {status} {text}").into())
                }
            }
        }
    };
    open(operator.clone(), "1010".into()).await?;
    for n in 0..WALLETS {
        open(client.clone(), wallet(n)).await?;
        let deposit = json!({"accounts": {"customer_funds": "1010", "customer_wallet": wallet(n)},
                             "amounts": {"amount": etb(DEPOSIT)}});
        let key = format!("deposit-{n}");
        let path = "/v1/operations/deposit_confirmed.v1";
        if !matches!(
            post(&http, base, path, &client, &key, &deposit).await,
            Answer::Posted { .. }
        ) {
            return Err(format!("the deposit to {} failed", wallet(n)).into());
        }
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
    odd: u64,
}

async fn worker(
    base: String,
    token: String,
    log: Log,
    cycle: u64,
    id: u64,
    deadline: Instant,
) -> Result<Counts> {
    let http = http()?;
    let mut rng = Rng(cycle * 1_000 + id);
    let mut counts = Counts::default();
    let mut n = 0u64;
    while Instant::now() < deadline {
        n += 1;
        let key = format!("c{cycle}-w{id}-{n}");
        let from = rng.next(WALLETS);
        let to = (from + 1 + rng.next(WALLETS - 1)) % WALLETS;
        // One in 20 asks for more than a wallet can hold, to be refused.
        let amount = if rng.next(20) == 0 {
            WALLETS as i64 * DEPOSIT + 1
        } else {
            1 + rng.next(5_000) as i64
        };
        log.write(&format!("sent {key} {from} {to} {amount}"))?;
        counts.sent += 1;
        let path = "/v1/operations/p2p_transfer.v1";
        match post(
            &http,
            &base,
            path,
            &token,
            &key,
            &transfer(from, to, amount),
        )
        .await
        {
            Answer::Posted { entry, .. } => {
                log.write(&format!("ack {key} {entry}"))?;
                counts.acked += 1;
            }
            Answer::Refused(code) => {
                log.write(&format!("rejected {key} {code}"))?;
                counts.rejected += 1;
            }
            Answer::Unknown(reason) => {
                log.write(&format!("failed {key} {reason}"))?;
                counts.failed += 1;
                // Something is down: do not spin.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Answer::Odd(status, code) => {
                log.write(&format!("odd {key} {status} {code}"))?;
                counts.odd += 1;
            }
        }
    }
    Ok(counts)
}

/// Sends transfers from `workers` workers for `seconds` seconds, whatever
/// happens to the database or the service meanwhile.
async fn load(base: &str, path: &str, cycle: u64, workers: u64, seconds: u64) -> Result<()> {
    let log = Log::open(path)?;
    let client = token(&http()?, base, CLIENT).await?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let tasks: Vec<_> = (0..workers)
        .map(|id| {
            let worker = worker(
                base.to_owned(),
                client.clone(),
                log.clone(),
                cycle,
                id,
                deadline,
            );
            tokio::spawn(worker)
        })
        .collect();
    let mut total = Counts::default();
    for task in tasks {
        let counts = task.await??;
        total.sent += counts.sent;
        total.acked += counts.acked;
        total.rejected += counts.rejected;
        total.failed += counts.failed;
        total.odd += counts.odd;
    }
    println!(
        "LOAD cycle={cycle} sent={} acked={} rejected={} failed={} odd={}",
        total.sent, total.acked, total.rejected, total.failed, total.odd
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
    /// An answer a well-formed request should never get.
    Odd(String),
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
        fn known<'a>(
            requests: &'a mut BTreeMap<String, Sent>,
            key: &str,
            line: &str,
        ) -> Result<&'a mut Known> {
            Ok(&mut requests
                .get_mut(key)
                .ok_or_else(|| format!("answer before sent: {line}"))?
                .known)
        }
        match *event {
            "sent" => {
                let [_, _, from, to, amount] = words[..] else {
                    return Err(format!("bad log line: {line}").into());
                };
                requests.insert(
                    key.clone(),
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
                *known(&mut requests, &key, &line)? = Known::Acked(id);
            }
            "rejected" | "retry-rejected" => *known(&mut requests, &key, &line)? = Known::Rejected,
            "odd" => *known(&mut requests, &key, &line)? = Known::Odd(words[2..].join(" ")),
            // No answer, or 5xx: the outcome stays unknown.
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
    .bind(CLIENT_ID.parse::<Uuid>()?)
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
/// unknown ones by retrying them through the API, and checks again.
async fn check(url: &str, base: &str, path: &str, cycle: &str) -> Result<bool> {
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
            (Known::Odd(answer), _) => {
                violations.push(format!("{key}: unexpected answer {answer}"));
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

    // Resolve each unknown outcome as a client would: retry it, same key.
    let http = http()?;
    let client = token(&http, base, CLIENT).await?;
    let route = "/v1/operations/p2p_transfer.v1";
    for (key, sent, existing) in &unknown {
        let body = transfer(sent.from, sent.to, sent.amount);
        match (
            post(&http, base, route, &client, key, &body).await,
            existing,
        ) {
            (Answer::Posted { entry, replayed }, Some(id)) if entry == id.to_string() => {
                if !replayed {
                    violations.push(format!("{key}: the retry of {id} was not marked replayed"));
                }
                log.write(&format!("retry-ack {key} {id}"))?;
            }
            (Answer::Posted { entry, .. }, Some(id)) => violations.push(format!(
                "{key}: committed as {id}, but the retry returned {entry}"
            )),
            (Answer::Posted { entry, .. }, None) => {
                log.write(&format!("retry-ack {key} {entry}"))?;
            }
            (Answer::Refused(code), None) => {
                log.write(&format!("retry-rejected {key} {code}"))?;
            }
            (Answer::Refused(code), Some(id)) => violations.push(format!(
                "{key}: committed as {id}, but the retry was refused: {code}"
            )),
            (Answer::Unknown(reason), _) => {
                violations.push(format!("{key}: the retry failed: {reason}"));
            }
            (Answer::Odd(status, code), _) => {
                violations.push(format!("{key}: the retry got {status} {code}"));
            }
        }
    }

    // Now every request has had its effect exactly once.
    let entries = journal(&pool, &prefix).await?;
    for (key, sent) in read_log(path, &prefix)? {
        let found = entries.get(&key).map(Vec::len).unwrap_or_default();
        let expected = match sent.known {
            Known::Acked(_) => 1,
            Known::Rejected => 0,
            // Already counted above.
            Known::Odd(_) => continue,
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
            .ok_or_else(|| "missing argument; see the usage in api_chaos.rs".into())
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
        "check" => check(arg(2)?, arg(3)?, arg(4)?, arg(5)?).await,
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

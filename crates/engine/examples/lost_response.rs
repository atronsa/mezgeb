//! Crash-after-commit demonstration for step 1.11: a process posts a
//! payment, the posting commits, and the process is killed before it can
//! answer. A new process retries the same request with the same key.
//!
//! ```sh
//! export URL=postgres://mezgeb_app:mezgeb_app@localhost:55432/mezgeb
//! cargo build --release --example lost_response
//! target/release/examples/lost_response setup "$URL"
//! timeout -s KILL 5 target/release/examples/lost_response post-and-hang "$URL" demo-1
//! target/release/examples/lost_response retry "$URL" demo-1
//! ```
//!
//! `post-and-hang` posts, then waits forever *before* printing the result,
//! as if the response were lost: it is killed after the commit. `retry`
//! sends the same request again and prints what came back.

use std::process::ExitCode;
use std::time::Duration;

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, IdempotencyKey, Ledger, Money,
};
use mezgeb_engine::{PgStore, PostRequest, post_entry};
use sqlx::{Connection, PgConnection};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const CHART: &str = r#"
name = "Lost response chart"
currency = "ETB"

[[accounts]]
code = 1020
name = "Bank: demo"
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

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn bank() -> Result<AccountId> {
    Ok(uuid7(0xb0_0001).parse()?)
}

fn wallet() -> Result<AccountId> {
    Ok(uuid7(0xb0_0002).parse()?)
}

fn client() -> Result<ClientId> {
    Ok(uuid7(0xc11e_0001).parse()?)
}

/// A new entry ID per process: a retry is a new attempt.
fn fresh_id() -> Result<EntryId> {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros();
    Ok(uuid7(u64::try_from(micros % (1 << 47))?).parse()?)
}

fn deposit(key: &str) -> Result<PostRequest> {
    Ok(PostRequest {
        entry_id: fresh_id()?,
        client_id: client()?,
        idempotency_key: IdempotencyKey::new(key)?,
        draft: EntryDraft::default()
            .debit(bank()?, Money::from_minor(12_345, Currency::ETB))
            .credit(wallet()?, Money::from_minor(12_345, Currency::ETB)),
        reverses: None,
    })
}

async fn count(conn: &mut PgConnection, key: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM journal_entries WHERE client_id = $1 AND idempotency_key = $2",
    )
    .bind(client()?.as_uuid())
    .bind(key)
    .fetch_one(conn)
    .await?)
}

async fn run(command: &str, url: &str, key: Option<&str>) -> Result<()> {
    match (command, key) {
        ("setup", _) => {
            let pool = sqlx::PgPool::connect(url).await?;
            let mut store = PgStore::new(pool, client()?);
            let chart = Chart::from_toml(CHART)?;
            for (id, reference) in [(bank()?, "1020"), (wallet()?, "2010:cust-demo")] {
                if store.get_account(id).await?.is_none() {
                    store.open_account(id, &chart, reference).await?;
                }
            }
            println!("accounts ready");
        }
        ("post-and-hang", Some(key)) => {
            let mut conn = PgConnection::connect(url).await?;
            let posted = post_entry(&mut conn, deposit(key)?).await?;
            // Committed. The response is never delivered: the process hangs
            // here until it is killed.
            tokio::time::sleep(Duration::from_secs(3_600)).await;
            println!("unreachable: {}", posted.id());
        }
        ("retry", Some(key)) => {
            let mut conn = PgConnection::connect(url).await?;
            let before = count(&mut conn, key).await?;
            let request = deposit(key)?;
            let new_id = request.entry_id;
            let posted = post_entry(&mut conn, request).await?;
            let after = count(&mut conn, key).await?;
            println!("entries with key {key:?} before the retry: {before}");
            println!("this attempt's new entry ID:  {new_id}");
            println!("entry returned by the retry:  {}", posted.id());
            println!("entries with key {key:?} after the retry:  {after}");
        }
        _ => return Err("usage: lost_response <setup|post-and-hang|retry> <url> [key]".into()),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(command), Some(url)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: lost_response <setup|post-and-hang|retry> <url> [key]");
        return ExitCode::from(2);
    };
    match run(command, url, args.get(3).map(String::as_str)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

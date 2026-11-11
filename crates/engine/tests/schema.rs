//! Database tests for schema v1 (step 1.08).
//!
//! Each test creates its own database, owned by `mezgeb_owner`, runs the
//! migrations as `mezgeb_owner`, then acts as `mezgeb_app`, the role the
//! service runs as. They need PostgreSQL with the roles from
//! `db/init/01-roles.sql`:
//!
//! ```sh
//! docker compose up -d
//! MEZGEB_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres \
//!     cargo test -p mezgeb-engine
//! ```
//!
//! The URL is a superuser's, used only to create and drop the throwaway
//! databases. Role passwords default to the role names (the local and CI
//! setup) and can be overridden with `MEZGEB_TEST_<ROLE>_PASSWORD`.
//!
//! Without `MEZGEB_TEST_DATABASE_URL` each test prints that it was skipped
//! and passes, except on CI (where `CI` is set), where it fails.

use std::sync::atomic::{AtomicU32, Ordering};

use mezgeb_engine::MIGRATOR;
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, Executor, PgConnection, Row};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CLIENT: &str = "00000000-0000-7000-8000-00000000c11e";

/// A UUIDv7-shaped ID built from a number.
fn id(n: u32) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

/// One throwaway database, migrated, with connections for each role.
struct TestDb {
    name: String,
    admin: PgConnectOptions,
}

impl TestDb {
    async fn create() -> Result<Option<TestDb>, Box<dyn std::error::Error>> {
        let Ok(url) = std::env::var("MEZGEB_TEST_DATABASE_URL") else {
            // On CI the database tests must run, never be skipped silently.
            if std::env::var_os("CI").is_some() {
                return Err("MEZGEB_TEST_DATABASE_URL must be set on CI".into());
            }
            eprintln!("skipped: MEZGEB_TEST_DATABASE_URL is not set");
            return Ok(None);
        };
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "mezgeb_test_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let admin: PgConnectOptions = url.parse()?;
        let mut conn = PgConnection::connect_with(&admin).await?;
        exec(
            &mut conn,
            format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        )
        .await?;
        exec(
            &mut conn,
            format!("CREATE DATABASE {name} OWNER mezgeb_owner"),
        )
        .await?;
        let db = TestDb { name, admin };
        MIGRATOR.run(&mut db.owner().await?).await?;
        Ok(Some(db))
    }

    async fn connect(&self, role: &str) -> Result<PgConnection, sqlx::Error> {
        let password = std::env::var(format!("MEZGEB_TEST_{}_PASSWORD", role.to_uppercase()))
            .unwrap_or_else(|_| role.to_owned());
        let options = self
            .admin
            .clone()
            .database(&self.name)
            .username(role)
            .password(&password);
        PgConnection::connect_with(&options).await
    }

    async fn owner(&self) -> Result<PgConnection, sqlx::Error> {
        self.connect("mezgeb_owner").await
    }

    async fn app(&self) -> Result<PgConnection, sqlx::Error> {
        self.connect("mezgeb_app").await
    }

    async fn readonly(&self) -> Result<PgConnection, sqlx::Error> {
        self.connect("mezgeb_readonly").await
    }

    async fn drop(self) -> Result<(), sqlx::Error> {
        let mut conn = PgConnection::connect_with(&self.admin).await?;
        exec(
            &mut conn,
            format!("DROP DATABASE {} WITH (FORCE)", self.name),
        )
        .await?;
        Ok(())
    }
}

/// Runs one SQL statement that the test builds at run time. Only test
/// constants and generated IDs go into it, never outside input.
async fn exec(conn: &mut PgConnection, sql: String) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(conn)
        .await
        .map(|_| ())
}

/// The SQLSTATE of a database error, such as `23514` or `MZ002`.
fn sqlstate<T>(result: Result<T, sqlx::Error>) -> Option<String> {
    result
        .err()?
        .as_database_error()?
        .code()
        .map(|c| c.into_owned())
}

async fn open(
    conn: &mut PgConnection,
    n: u32,
    code: i16,
    sub: Option<&str>,
    account_type: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO accounts (id, code, sub_account, name, account_type, currency)
         VALUES ($1::uuid, $2, $3, 'Test account', $4, 'ETB')",
    )
    .bind(id(n))
    .bind(code)
    .bind(sub)
    .bind(account_type)
    .execute(conn)
    .await
    .map(|_| ())
}

/// Posts an entry the way the engine will: update each account's totals,
/// then insert the line with the totals after it. Does not commit.
async fn post_lines(
    conn: &mut PgConnection,
    entry: u32,
    key: &str,
    reverses: Option<u32>,
    lines: &[(u32, &str, i64)],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO journal_entries (id, client_id, idempotency_key, reverses_entry_id)
         VALUES ($1::uuid, $2::uuid, $3, $4::uuid)",
    )
    .bind(id(entry))
    .bind(CLIENT)
    .bind(key)
    .bind(reverses.map(id))
    .execute(&mut *conn)
    .await?;
    for (line_no, (account, side, amount)) in lines.iter().enumerate() {
        let (debit, credit) = if *side == "debit" {
            (*amount, 0)
        } else {
            (0, *amount)
        };
        let totals = sqlx::query(
            "UPDATE accounts
                SET debits_total = debits_total + $2,
                    credits_total = credits_total + $3,
                    version = version + 1
              WHERE id = $1::uuid
          RETURNING debits_total, credits_total",
        )
        .bind(id(*account))
        .bind(debit)
        .bind(credit)
        .fetch_one(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO journal_lines
                 (entry_id, line_no, account_id, side, amount, currency, debits_after, credits_after)
             VALUES ($1::uuid, $2, $3::uuid, $4, $5, 'ETB', $6, $7)",
        )
        .bind(id(entry))
        .bind(line_no as i16)
        .bind(id(*account))
        .bind(*side)
        .bind(*amount)
        .bind(totals.try_get::<i64, _>("debits_total")?)
        .bind(totals.try_get::<i64, _>("credits_total")?)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Posts and commits an entry in its own transaction.
async fn post(
    conn: &mut PgConnection,
    entry: u32,
    reverses: Option<u32>,
    lines: &[(u32, &str, i64)],
) -> Result<(), sqlx::Error> {
    let mut tx = conn.begin().await?;
    post_lines(&mut tx, entry, &format!("key-{entry}"), reverses, lines).await?;
    tx.commit().await
}

/// Opens the bank (1) and two wallets (2, 3), and deposits 1,000 santim to
/// wallet 2 as entry 1.
async fn funded(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    open(conn, 1, 1010, None, "asset").await?;
    open(conn, 2, 2010, Some("cust-abebe"), "liability").await?;
    open(conn, 3, 2010, Some("cust-hana"), "liability").await?;
    post(conn, 1, None, &[(1, "debit", 1_000), (2, "credit", 1_000)]).await
}

async fn balance(conn: &mut PgConnection, account: u32) -> Result<(i64, i64), sqlx::Error> {
    let row = sqlx::query("SELECT debits_total, credits_total FROM accounts WHERE id = $1::uuid")
        .bind(id(account))
        .fetch_one(conn)
        .await?;
    Ok((row.try_get("debits_total")?, row.try_get("credits_total")?))
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_valid_posting_commits() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    assert_eq!(balance(&mut app, 1).await?, (1_000, 0));
    assert_eq!(balance(&mut app, 2).await?, (0, 1_000));
    post(&mut app, 2, None, &[(2, "debit", 300), (3, "credit", 300)]).await?;
    assert_eq!(balance(&mut app, 2).await?, (300, 1_000));
    let sequences: Vec<i64> =
        sqlx::query_scalar("SELECT sequence FROM journal_entries ORDER BY sequence")
            .fetch_all(&mut app)
            .await?;
    assert_eq!(sequences.len(), 2);
    assert!(sequences[0] < sequences[1], "INV-12: posting order");
    db.drop().await?;
    Ok(())
}

/// INV-1, INV-4: the deferred trigger rejects an unbalanced entry, an entry
/// with one line, and an entry with no lines, at commit.
#[tokio::test]
async fn unbalanced_entries_fail_at_commit() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;

    for (entry, lines) in [
        (2, vec![(1, "debit", 100), (2, "credit", 99)]),
        (3, vec![(1, "debit", 100)]),
        (4, vec![]),
    ] {
        let mut tx = app.begin().await?;
        // Every statement succeeds; the entry is only checked at commit.
        post_lines(&mut tx, entry, &format!("k{entry}"), None, &lines).await?;
        assert_eq!(
            sqlstate(tx.commit().await).as_deref(),
            Some("MZ002"),
            "entry {entry}"
        );
    }
    assert_eq!(
        balance(&mut app, 1).await?,
        (1_000, 0),
        "nothing was posted"
    );
    db.drop().await?;
    Ok(())
}

/// INV-1: a zero or negative amount is rejected immediately.
#[tokio::test]
async fn line_amounts_must_be_positive() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    let mut tx = app.begin().await?;
    let result = post_lines(&mut tx, 2, "k", None, &[(1, "debit", 0), (2, "credit", 0)]).await;
    assert_eq!(sqlstate(result).as_deref(), Some("23514"));
    db.drop().await?;
    Ok(())
}

/// INV-9: the service's role has no UPDATE, DELETE or TRUNCATE privilege on
/// the journal, and the triggers stop even the owner.
#[tokio::test]
async fn journal_history_cannot_change() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;

    let attempts = [
        "UPDATE journal_lines SET amount = 1",
        "UPDATE journal_entries SET metadata = '{\"x\": 1}'",
        "DELETE FROM journal_lines",
        "DELETE FROM journal_entries",
        "TRUNCATE journal_lines",
        "TRUNCATE journal_entries CASCADE",
    ];
    for sql in attempts {
        assert_eq!(
            sqlstate(app.execute(sql).await).as_deref(),
            Some("42501"),
            "app: {sql}"
        );
    }
    let mut owner = db.owner().await?;
    for sql in attempts {
        assert_eq!(
            sqlstate(owner.execute(sql).await).as_deref(),
            Some("MZ001"),
            "owner: {sql}"
        );
    }
    db.drop().await?;
    Ok(())
}

/// The service may change only totals, version and status of an account.
#[tokio::test]
async fn the_app_role_cannot_change_account_identity() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    for sql in [
        "UPDATE accounts SET name = 'Renamed'",
        "UPDATE accounts SET allow_negative = true",
        "UPDATE accounts SET code = 1020",
        "DELETE FROM accounts",
    ] {
        assert_eq!(
            sqlstate(app.execute(sql).await).as_deref(),
            Some("42501"),
            "{sql}"
        );
    }
    exec(
        &mut app,
        format!(
            "UPDATE accounts SET status = 'frozen' WHERE id = '{}'",
            id(3)
        ),
    )
    .await?;

    // Even the owner cannot change identity columns.
    let mut owner = db.owner().await?;
    assert_eq!(
        sqlstate(owner.execute("UPDATE accounts SET code = 1020").await).as_deref(),
        Some("MZ005")
    );
    db.drop().await?;
    Ok(())
}

/// INV-8: the CHECK refuses a balance below zero on the normal side.
#[tokio::test]
async fn a_wallet_cannot_go_negative() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    let mut tx = app.begin().await?;
    let result = post_lines(
        &mut tx,
        2,
        "k",
        None,
        &[(2, "debit", 1_001), (3, "credit", 1_001)],
    )
    .await;
    assert_eq!(sqlstate(result).as_deref(), Some("23514"));
    tx.rollback().await?;
    // Spending exactly the balance is fine.
    post(
        &mut app,
        3,
        None,
        &[(2, "debit", 1_000), (3, "credit", 1_000)],
    )
    .await?;
    db.drop().await?;
    Ok(())
}

/// Running totals never go down.
#[tokio::test]
async fn totals_only_increase() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    let result = exec(
        &mut app,
        format!(
            "UPDATE accounts SET credits_total = 999 WHERE id = '{}'",
            id(2)
        ),
    )
    .await;
    assert_eq!(sqlstate(result).as_deref(), Some("MZ005"));
    db.drop().await?;
    Ok(())
}

/// INV-14: one reversal per entry, never of a reversal, always a mirror.
#[tokio::test]
async fn reversal_rules() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    let mirror = [(1, "credit", 1_000), (2, "debit", 1_000)];

    post(&mut app, 2, Some(1), &mirror).await?;

    // A second reversal of entry 1.
    let mut tx = app.begin().await?;
    let second = post_lines(&mut tx, 3, "k3", Some(1), &mirror).await;
    assert_eq!(sqlstate(second).as_deref(), Some("23505"));
    tx.rollback().await?;

    // A reversal of the reversal.
    let mut tx = app.begin().await?;
    let of_reversal = post_lines(
        &mut tx,
        4,
        "k4",
        Some(2),
        &[(1, "debit", 1_000), (2, "credit", 1_000)],
    )
    .await;
    assert_eq!(sqlstate(of_reversal).as_deref(), Some("MZ003"));
    tx.rollback().await?;

    // A "reversal" that does not mirror its original.
    post(&mut app, 5, None, &[(1, "debit", 500), (3, "credit", 500)]).await?;
    let mut tx = app.begin().await?;
    post_lines(
        &mut tx,
        6,
        "k6",
        Some(5),
        &[(1, "credit", 400), (3, "debit", 400)],
    )
    .await?;
    assert_eq!(sqlstate(tx.commit().await).as_deref(), Some("MZ003"));
    db.drop().await?;
    Ok(())
}

/// INV-13: an idempotency key is used once per client, and well formed.
#[tokio::test]
async fn idempotency_keys_are_unique_per_client() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    let mut tx = app.begin().await?;
    let reused = post_lines(
        &mut tx,
        2,
        "key-1",
        None,
        &[(1, "debit", 1), (2, "credit", 1)],
    )
    .await;
    assert_eq!(sqlstate(reused).as_deref(), Some("23505"));
    tx.rollback().await?;
    let mut tx = app.begin().await?;
    let malformed = post_lines(
        &mut tx,
        3,
        "has space",
        None,
        &[(1, "debit", 1), (2, "credit", 1)],
    )
    .await;
    assert_eq!(sqlstate(malformed).as_deref(), Some("23514"));
    db.drop().await?;
    Ok(())
}

/// INV-3 and INV-4: a line's currency is its account's, and a line always
/// belongs to an existing entry.
#[tokio::test]
async fn lines_reference_their_entry_and_account_currency() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;

    let insert_line = |entry: String, currency: &'static str| {
        format!(
            "INSERT INTO journal_lines
                 (entry_id, line_no, account_id, side, amount, currency, debits_after, credits_after)
             VALUES ('{entry}', 9, '{}', 'debit', 1, '{currency}', 0, 0)",
            id(1)
        )
    };
    // An orphan line: its entry does not exist.
    let orphan = exec(&mut app, insert_line(id(99), "ETB")).await;
    assert_eq!(sqlstate(orphan).as_deref(), Some("23503"));

    // A USD line on an ETB account.
    let mut tx = app.begin().await?;
    let usd = exec(&mut tx, insert_line(id(1), "USD")).await;
    assert_eq!(sqlstate(usd).as_deref(), Some("23503"));
    db.drop().await?;
    Ok(())
}

/// INV-24 and the account lifecycle: no posting to a frozen or closed
/// account, closing only at zero, never reopening.
#[tokio::test]
async fn account_status_rules() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut app = db.app().await?;
    funded(&mut app).await?;
    let set = |account: u32, status: &str| {
        format!(
            "UPDATE accounts SET status = '{status}' WHERE id = '{}'",
            id(account)
        )
    };

    // Closing a wallet that still holds money.
    assert_eq!(
        sqlstate(exec(&mut app, set(2, "closed")).await).as_deref(),
        Some("23514")
    );

    // Posting to a frozen account.
    exec(&mut app, set(3, "frozen")).await?;
    let mut tx = app.begin().await?;
    let frozen = post_lines(&mut tx, 2, "k2", None, &[(2, "debit", 1), (3, "credit", 1)]).await;
    assert_eq!(sqlstate(frozen).as_deref(), Some("MZ004"));
    tx.rollback().await?;

    // Closing at zero is fine; reopening is not.
    exec(&mut app, set(3, "closed")).await?;
    assert_eq!(
        sqlstate(exec(&mut app, set(3, "open")).await).as_deref(),
        Some("MZ005")
    );
    db.drop().await?;
    Ok(())
}

/// The read-only role reads and nothing else.
#[tokio::test]
async fn the_readonly_role_only_reads() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    funded(&mut db.app().await?).await?;
    let mut readonly = db.readonly().await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM journal_lines")
        .fetch_one(&mut readonly)
        .await?;
    assert_eq!(count, 2);
    assert_eq!(
        sqlstate(open(&mut readonly, 9, 4010, None, "revenue").await).as_deref(),
        Some("42501")
    );
    db.drop().await?;
    Ok(())
}

/// Migrations run up, down and up again on an empty database.
#[tokio::test]
async fn migrations_go_up_down_and_up() -> TestResult {
    let Some(db) = TestDb::create().await? else {
        return Ok(());
    };
    let mut owner = db.owner().await?;
    let tables = "SELECT count(*) FROM information_schema.tables
                   WHERE table_schema = 'public' AND table_name <> '_sqlx_migrations'";
    assert_eq!(
        sqlx::query_scalar::<_, i64>(tables)
            .fetch_one(&mut owner)
            .await?,
        4
    );
    MIGRATOR.undo(&mut owner, 0).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(tables)
            .fetch_one(&mut owner)
            .await?,
        0
    );
    MIGRATOR.run(&mut owner).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(tables)
            .fetch_one(&mut owner)
            .await?,
        4
    );
    drop(owner);
    db.drop().await?;
    Ok(())
}

//! Test harness for database tests: throwaway databases on a real
//! PostgreSQL, one per test (or per test case).
//!
//! They need PostgreSQL with the roles from `db/init/01-roles.sql`:
//!
//! ```sh
//! docker compose up -d
//! MEZGEB_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres \
//!     cargo test -p mezgeb-engine
//! ```
//!
//! The URL is a superuser's, used only to create and drop the throwaway
//! databases. Migrations run as `mezgeb_owner`; tests act as `mezgeb_app`.
//! Role passwords default to the role names (the local and CI setup) and can
//! be overridden with `MEZGEB_TEST_<ROLE>_PASSWORD`.
//!
//! Without `MEZGEB_TEST_DATABASE_URL` a test is skipped, except on CI (where
//! `CI` is set), where it fails.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};

use mezgeb_engine::MIGRATOR;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The PostgreSQL server the tests run on.
#[derive(Debug, Clone)]
pub struct Server {
    admin: PgConnectOptions,
}

impl Server {
    /// The server from `MEZGEB_TEST_DATABASE_URL`, or `None` to skip.
    pub fn from_env() -> Result<Option<Server>, Box<dyn std::error::Error>> {
        let Ok(url) = std::env::var("MEZGEB_TEST_DATABASE_URL") else {
            // On CI the database tests must run, never be skipped silently.
            if std::env::var_os("CI").is_some() {
                return Err("MEZGEB_TEST_DATABASE_URL must be set on CI".into());
            }
            eprintln!("skipped: MEZGEB_TEST_DATABASE_URL is not set");
            return Ok(None);
        };
        Ok(Some(Server {
            admin: url.parse()?,
        }))
    }

    fn unique_name(prefix: &str) -> String {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        format!(
            "{prefix}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// A new database owned by `mezgeb_owner`, empty or copied from a
    /// template.
    async fn create(&self, template: Option<&str>) -> Result<TestDb, sqlx::Error> {
        let name = Server::unique_name("mezgeb_test");
        let mut conn = PgConnection::connect_with(&self.admin).await?;
        let template = template
            .map(|t| format!(" TEMPLATE {t}"))
            .unwrap_or_default();
        exec(
            &mut conn,
            format!("CREATE DATABASE {name} OWNER mezgeb_owner{template}"),
        )
        .await?;
        conn.close().await?;
        Ok(TestDb {
            name,
            admin: self.admin.clone(),
        })
    }

    /// A new, migrated database.
    pub async fn migrated(&self) -> Result<TestDb, Box<dyn std::error::Error>> {
        let db = self.create(None).await?;
        let mut owner = db.owner().await?;
        MIGRATOR.run(&mut owner).await?;
        owner.close().await?;
        Ok(db)
    }

    /// A migrated database to copy with [`Server::copy_of`]: much
    /// faster than migrating every time.
    pub async fn template(&self) -> Result<TestDb, Box<dyn std::error::Error>> {
        self.migrated().await
    }

    /// A new database copied from `template`.
    pub async fn copy_of(&self, template: &TestDb) -> Result<TestDb, sqlx::Error> {
        self.create(Some(&template.name)).await
    }
}

/// One throwaway database.
#[derive(Debug)]
pub struct TestDb {
    name: String,
    admin: PgConnectOptions,
}

impl TestDb {
    fn options(&self, role: &str) -> PgConnectOptions {
        let password = std::env::var(format!("MEZGEB_TEST_{}_PASSWORD", role.to_uppercase()))
            .unwrap_or_else(|_| role.to_owned());
        self.admin
            .clone()
            .database(&self.name)
            .username(role)
            .password(&password)
    }

    pub async fn owner(&self) -> Result<PgConnection, sqlx::Error> {
        PgConnection::connect_with(&self.options("mezgeb_owner")).await
    }

    pub async fn app(&self) -> Result<PgConnection, sqlx::Error> {
        PgConnection::connect_with(&self.options("mezgeb_app")).await
    }

    pub async fn readonly(&self) -> Result<PgConnection, sqlx::Error> {
        PgConnection::connect_with(&self.options("mezgeb_readonly")).await
    }

    /// A connection pool as `mezgeb_app`, the service's role.
    pub async fn app_pool(&self) -> Result<PgPool, sqlx::Error> {
        PgPoolOptions::new()
            .max_connections(2)
            .connect_with(self.options("mezgeb_app"))
            .await
    }

    /// Drops the database, closing any connections still open to it.
    pub async fn drop(self) -> Result<(), sqlx::Error> {
        let mut conn = PgConnection::connect_with(&self.admin).await?;
        exec(
            &mut conn,
            format!("DROP DATABASE {} WITH (FORCE)", self.name),
        )
        .await?;
        conn.close().await
    }
}

/// Runs one SQL statement that the test builds at run time. Only test
/// constants and generated IDs go into it, never outside input.
pub async fn exec(conn: &mut PgConnection, sql: String) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(conn)
        .await
        .map(|_| ())
}

/// A UUIDv7-shaped ID string built from a number.
pub fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

//! PostgreSQL storage (spec §8).
//!
//! Applies `mezgeb_core::Engine` inside one database transaction per batch.
//! Database constraints and triggers are a second line of defence, never the
//! only one.

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub struct PgStore {
    pub pool: sqlx::PgPool,
}

//! Posting engine (ADR-0002, layer 1).
//!
//! Knows accounts, entries, lines, holds, reversals and periods, and enforces
//! accounting rules and nothing else. Each posting is one PostgreSQL
//! transaction (ADR-0008) that locks its accounts in ascending id order
//! (ADR-0007). Database constraints and triggers are a second line of
//! defence, never the only one.
//!
//! Migrations live in the top-level `migrations/` directory.

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub struct PgStore {
    pub pool: sqlx::PgPool,
}

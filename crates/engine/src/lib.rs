//! Posting engine (ADR-0002, layer 1).
//!
//! Knows accounts, entries, lines, holds, reversals and periods, and enforces
//! accounting rules and nothing else. Each posting is one PostgreSQL
//! transaction (ADR-0008) that locks its accounts in ascending id order
//! (ADR-0007). Database constraints and triggers are a second line of
//! defence, never the only one.
//!
//! Migrations live in the top-level `migrations/` directory.

mod concurrency;
mod holds;
mod post;
mod rows;
mod store;
pub mod verification;
pub mod verify;

pub use concurrency::{FaultPoint, Isolation, PostOptions, RetryStats};
pub use holds::{
    CaptureHold, HoldRequest, PlaceHold, all_holds, capture_hold, expire_due_holds, expire_hold,
    place_hold, release_hold,
};
pub use post::{
    PostRequest, lookup_by_idempotency_key, post_entry, post_entry_with, reverse_entry,
    reverse_entry_with,
};
pub use store::PgStore;
pub use verification::{Finding, Scope, VerificationReport, VerifyOptions, verify};
pub use verify::{IntegrityReport, integrity_report};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

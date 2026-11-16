//! Posting engine (ADR-0002, layer 1).
//!
//! Knows accounts, entries, lines, holds, reversals and periods, and enforces
//! accounting rules and nothing else. Each posting is one PostgreSQL
//! transaction (ADR-0008) that locks its accounts in ascending id order
//! (ADR-0007). Database constraints and triggers are a second line of
//! defence, never the only one.
//!
//! Migrations live in the top-level `migrations/` directory.

mod accounts;
mod adjustments;
mod ageing;
mod clients;
mod close;
mod concurrency;
mod entries;
mod holds;
mod offline;
mod periods;
mod post;
mod reports;
mod rows;
pub mod signing;
mod store;
pub mod tamper;
pub mod verification;
pub mod verify;

pub use accounts::{
    Opened, account_by_reference, account_opener, get_account, list_accounts, open_account_keyed,
    totals_at,
};
pub use adjustments::{
    AdjustmentRequest, AdjustmentStatus, NewAdjustment, Outcome, approve_adjustment,
    get_adjustment, reject_adjustment, request_adjustment,
};
pub use ageing::{OpenItem, ageing};
pub use clients::{
    Client, ClientError, Credential, EXCLUSIVE_ROLES, ROLES, TokenOwner, add_credential,
    check_roles, client_by_name, create_client, credential_for_login, disable_client,
    generate_secret, generate_token, hash_secret, list_clients, list_credentials,
    purge_expired_tokens, revoke_credential, store_token, token_digest, token_owner, verify_secret,
};
pub use close::{
    CloseReport, PeriodFinding, Snapshot, begin_close, close_period, finish_close, period_snapshots,
};
pub use concurrency::{FaultPoint, Isolation, PostOptions, RetryStats};
pub use entries::{EntryRecord, entry_record, entry_record_by_key};
pub use holds::{
    CaptureHold, HoldRequest, PlaceHold, all_holds, capture_hold, expire_due_holds, expire_hold,
    get_hold, hold_of, place_hold, release_hold,
};
pub use offline::{
    Attestation, AttestationKind, Device, DeviceStatus, EscrowFinding, IssuedVoucher, IssuerKey,
    NewDevice, NewVoucher, OfflineError, Registered, VoucherStatus, escrow_findings, get_device,
    get_voucher, issue_voucher, register_device, set_device_status, set_tier_limits, tier_limits,
};
pub use periods::{Period, PeriodStatus, all_periods, current_period};
pub use post::{
    PostRequest, lookup_by_idempotency_key, post_entry, post_entry_with, reverse_entry,
    reverse_entry_with,
};
pub use reports::{
    AsOf, GeneralLedger, GeneralLedgerLine, LedgerCursor, LedgerPage, TrialBalance,
    TrialBalanceRow, general_ledger, general_ledger_page, trial_balance,
};
pub use signing::Signer;
pub use store::PgStore;
pub use tamper::{
    AuditEvent, ChainReport, Checkpoint, NewAuditEvent, Receipt, append_audit, audit_events,
    checkpoints, receipt, record_checkpoint, signing_keys, verify_chains,
};
pub use verification::{Finding, Scope, VerificationReport, VerifyOptions, verify};
pub use verify::{IntegrityReport, integrity_report};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

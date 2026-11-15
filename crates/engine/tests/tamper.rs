//! Tamper evidence on PostgreSQL (step 2.09, ADR-0018): the hash chain,
//! receipts, audit events and checkpoints, and what a superuser's tampering
//! leaves behind (T-11a, T-11b, T-26a, T-26b).

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use std::sync::Arc;

use mezgeb_core::chain::receipt_message;
use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, IdempotencyKey, Ledger, Money,
    Timestamp,
};
use mezgeb_engine::signing::verify_signature;
use mezgeb_engine::{
    Checkpoint, NewAuditEvent, PgStore, PostOptions, PostRequest, Signer, append_audit,
    post_entry_with, receipt, record_checkpoint, signing_keys, verify_chains,
};
use support::{Server, TestDb, TestResult, exec, uuid7};
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error>;

fn account(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn client() -> Result<ClientId, mezgeb_core::IdError> {
    uuid7(0xc11e).parse()
}

fn signer() -> Result<Arc<Signer>, String> {
    Ok(Arc::new(Signer::from_hex(&"5a".repeat(32))?))
}

fn options() -> Result<PostOptions, String> {
    Ok(PostOptions {
        signer: Some(signer()?),
        ..PostOptions::default()
    })
}

fn now() -> Timestamp {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    Timestamp::from_micros(i64::try_from(micros).unwrap_or(i64::MAX))
}

/// Posts `amount` from the bank to Abebe under `key`.
async fn post(db: &TestDb, key: &str, amount: i64) -> Result<EntryId, BoxError> {
    let mut conn = db.app().await?;
    let request = PostRequest::new(
        EntryId::from_uuid(Uuid::now_v7())?,
        client()?,
        IdempotencyKey::new(key)?,
        EntryDraft::default()
            .debit(account(1)?, Money::from_minor(amount, Currency::ETB))
            .credit(account(2)?, Money::from_minor(amount, Currency::ETB)),
    );
    Ok(post_entry_with(&mut conn, request, &options()?).await?.id())
}

fn audit(action: &str, outcome: &str) -> NewAuditEvent {
    NewAuditEvent {
        client: None,
        actor: Some("ops:maker".into()),
        action: action.into(),
        target: Some("entry:1".into()),
        request_id: Some("r-1".into()),
        outcome: outcome.into(),
        details: "{\"status\":201}".into(),
    }
}

/// A ledger with three chained, signed entries, two audit events and a
/// checkpoint of period 1. Returns the entries.
async fn setup(server: &Server) -> Result<(TestDb, Vec<EntryId>, Checkpoint), BoxError> {
    let db = server.migrated().await?;
    let mut store = PgStore::new(db.app_pool().await?, client()?);
    let chart = Chart::from_toml(common::CHART)?;
    store.open_account(account(1)?, &chart, "1010").await?;
    store
        .open_account(account(2)?, &chart, "2010:cust-abebe")
        .await?;
    // The service publishes its key before it signs anything.
    signer()?.register(&mut db.app().await?).await?;
    let mut entries = Vec::new();
    for (n, amount) in [(1, 1_000), (2, 250), (3, 75)] {
        entries.push(post(&db, &format!("k-{n}"), amount).await?);
    }
    let mut app = db.app().await?;
    append_audit(&mut app, &audit("operations.post", "ok"), now()).await?;
    append_audit(&mut app, &audit("operations.post", "refused"), now()).await?;
    let checkpoint = record_checkpoint(&mut app, 1, &*signer()?, now()).await?;
    Ok((db, entries, checkpoint))
}

/// Runs SQL as the superuser with triggers switched off: what an attacker
/// with full database access could do.
async fn tamper(db: &TestDb, sql: &str) -> Result<(), BoxError> {
    let mut conn = db.superuser().await?;
    exec(
        &mut conn,
        format!("SET session_replication_role = replica; {sql}"),
    )
    .await?;
    Ok(())
}

async fn findings(db: &TestDb, external: &[Checkpoint]) -> Result<Vec<String>, BoxError> {
    let mut conn = db.verifier().await?;
    Ok(verify_chains(&mut conn, external).await?.findings)
}

#[tokio::test]
async fn a_clean_ledger_verifies_and_every_receipt_is_signed() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, entries, checkpoint) = setup(&server).await?;
    let mut conn = db.verifier().await?;
    let report = verify_chains(&mut conn, std::slice::from_ref(&checkpoint)).await?;
    assert!(report.is_clean(), "{report}");
    assert_eq!(
        (report.entries, report.audit_events, report.unsigned),
        (3, 2, 0)
    );
    assert_eq!((checkpoint.position, checkpoint.audit_position), (3, 2));

    // T-26a: each entry has one receipt, verifiable with the public key.
    let keys = signing_keys(&mut conn).await?;
    for (n, entry) in entries.iter().enumerate() {
        let r = receipt(&mut conn, *entry).await?.ok_or("no receipt")?;
        assert_eq!(r.position, n as u64 + 1);
        let public = keys
            .get(r.key_id.as_deref().unwrap_or_default())
            .ok_or("no key")?;
        let message = receipt_message(r.position, r.entry, &r.hash);
        assert!(verify_signature(
            public,
            &message,
            r.signature.as_deref().unwrap_or_default()
        ));
    }
    // T-26b: a retry of the first request returns the identical receipt.
    let first = receipt(&mut conn, entries[0]).await?;
    let again = post(&db, "k-1", 1_000).await?;
    assert_eq!(again, entries[0]);
    assert_eq!(receipt(&mut conn, again).await?, first);
    assert!(verify_chains(&mut conn, &[]).await?.is_clean());
    drop(conn);
    db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn tampering_with_entries_lines_or_audit_events_is_found() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    // Each attack on a fresh copy of the same ledger.
    let attacks: [(&str, &str, &str); 7] = [
        (
            "a line's amount",
            "UPDATE journal_lines SET amount = amount + 1 WHERE line_no = 1
               AND entry_id = (SELECT entry_id FROM entry_chain WHERE position = 2)",
            "does not match its hash",
        ),
        (
            "an entry's metadata",
            "UPDATE journal_entries SET metadata = '{\"note\": \"forged\"}'
              WHERE id = (SELECT entry_id FROM entry_chain WHERE position = 1)",
            "does not match its hash",
        ),
        (
            "a line's account",
            "UPDATE journal_lines SET account_id = (SELECT id FROM accounts WHERE code = 1010)
              WHERE side = 'credit'
                AND entry_id = (SELECT entry_id FROM entry_chain WHERE position = 3)",
            "does not match its hash",
        ),
        (
            "a link in the middle",
            "DELETE FROM entry_chain WHERE position = 2",
            "links are missing",
        ),
        (
            "an audit event",
            "UPDATE audit_events SET outcome = 'ok' WHERE position = 2",
            "the event was changed",
        ),
        (
            "an entry slipped in without a link",
            "INSERT INTO journal_entries (id, client_id, idempotency_key, metadata, kind, period_id)
             VALUES ('00000000-0000-7000-8000-00000000f00d',
                     '00000000-0000-7000-8000-00000000c11e', 'slipped', '{}', 'standard', 1)",
            "is not in the chain",
        ),
        (
            "a receipt signature",
            "UPDATE entry_chain SET signature = decode(repeat('00', 64), 'hex') WHERE position = 1",
            "signature does not verify",
        ),
    ];
    let (template, _, _) = setup(&server).await?;
    for (name, sql, wanted) in attacks {
        let db = server.copy_of(&template).await?;
        assert!(findings(&db, &[]).await?.is_empty(), "{name}: clean before");
        tamper(&db, sql).await?;
        let found = findings(&db, &[]).await?;
        assert!(
            found.iter().any(|f| f.contains(wanted)),
            "{name}: expected a finding with {wanted:?}, found {found:?}"
        );
        db.drop_unverified().await?;
    }
    template.drop_unverified().await?;
    Ok(())
}

/// Deleting the last entry and moving the head back leaves a consistent
/// chain, which only a checkpoint can contradict: the one in the database,
/// or, if that is deleted too, a copy kept elsewhere.
#[tokio::test]
async fn deleting_the_last_entry_is_found_by_the_checkpoint() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let (db, entries, checkpoint) = setup(&server).await?;
    let last = entries[2];
    let rewind = format!(
        "DELETE FROM entry_chain WHERE position = 3;
         DELETE FROM journal_lines WHERE entry_id = '{last}';
         DELETE FROM idempotency_records WHERE entry_id = '{last}';
         DELETE FROM journal_entries WHERE id = '{last}';
         UPDATE chain_heads SET position = 2,
                hash = (SELECT hash FROM entry_chain WHERE position = 2)
          WHERE chain = 'entries';"
    );
    tamper(&db, &rewind).await?;
    let found = findings(&db, &[]).await?;
    assert!(
        found
            .iter()
            .any(|f| f.contains("no longer reaches position 3")),
        "{found:?}"
    );
    // The attacker deletes the checkpoint too: the copy kept outside the
    // database still catches it.
    tamper(&db, "DELETE FROM checkpoints").await?;
    assert!(
        findings(&db, &[]).await?.is_empty(),
        "nothing left inside to contradict it"
    );
    let found = findings(&db, &[checkpoint]).await?;
    assert!(
        found
            .iter()
            .any(|f| f.contains("(copy)") && f.contains("no longer reaches position 3")),
        "{found:?}"
    );
    db.drop_unverified().await?;
    Ok(())
}

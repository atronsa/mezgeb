//! Tamper evidence (ADR-0018, INV-11, INV-26): the hash chain over the
//! journal and its receipts, the audit events and their chain, the signed
//! checkpoints, and their verification.
//!
//! **Appending.** Every posting ends, inside its own transaction, by locking
//! the `entries` head, hashing the entry onto it ([`mezgeb_core::chain`]),
//! signing the result (the entry's receipt) and moving the head. Postings
//! therefore serialise for that last step, and chain order is commit order.
//! Audit events do the same on the `audit` head.
//!
//! **Verifying.** [`verify_chains`] recomputes both chains from the first
//! link, checks every signature against the published keys, checks that no
//! entry after the chain began is missing from it, and checks every
//! checkpoint, from the database and from copies kept elsewhere, against
//! the chains. What a superuser changes, deletes or rewrites after a
//! checkpoint someone else holds becomes a finding.

use std::collections::BTreeMap;
use std::fmt;

use mezgeb_core::chain::{
    ChainAudit, ChainEntry, ChainHash, GENESIS, audit_hash, checkpoint_message, entry_hash, hex,
    receipt_message,
};
use mezgeb_core::{ClientId, EntryId, LedgerError, PostedEntry, Timestamp};
use sqlx::postgres::PgRow;
use sqlx::{Acquire, PgConnection, Row};
use uuid::Uuid;

use crate::post::load_entry;
use crate::rows::{db, entry_id, storage};
use crate::signing::{Signer, verify_signature};

fn hash32(bytes: Vec<u8>) -> Result<ChainHash, LedgerError> {
    <ChainHash>::try_from(bytes.as_slice()).map_err(|_| storage("a stored hash is not 32 bytes"))
}

async fn lock_head(conn: &mut PgConnection, chain: &str) -> Result<(u64, ChainHash), LedgerError> {
    let row = sqlx::query("SELECT position, hash FROM chain_heads WHERE chain = $1 FOR UPDATE")
        .bind(chain)
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
    let position: i64 = row.try_get("position").map_err(db)?;
    Ok((
        u64::try_from(position).map_err(storage)?,
        hash32(row.try_get("hash").map_err(db)?)?,
    ))
}

async fn move_head(
    conn: &mut PgConnection,
    chain: &str,
    position: u64,
    hash: &ChainHash,
) -> Result<(), LedgerError> {
    sqlx::query("UPDATE chain_heads SET position = $2, hash = $3 WHERE chain = $1")
        .bind(chain)
        .bind(i64::try_from(position).map_err(storage)?)
        .bind(&hash[..])
        .execute(&mut *conn)
        .await
        .map_err(db)?;
    Ok(())
}

// ------------------------------------------------------------------ receipts

/// An entry's link in the chain. Its signature is the entry's ledger
/// receipt (INV-26): proof, verifiable with the public key alone, that the
/// ledger recorded this entry at this position with this hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub position: u64,
    pub entry: EntryId,
    pub prev_hash: ChainHash,
    pub hash: ChainHash,
    /// Absent only where the engine ran without a signing key (tests).
    pub key_id: Option<String>,
    pub signature: Option<Vec<u8>>,
}

/// What the database assigned to an entry, besides the entry itself.
pub(crate) struct Assigned<'a> {
    pub client: ClientId,
    pub idempotency_key: &'a str,
    pub recorded_at: Timestamp,
    pub period: Option<i64>,
}

/// Chains a posted entry, inside its posting transaction.
pub(crate) async fn append_entry(
    conn: &mut PgConnection,
    entry: &PostedEntry,
    assigned: &Assigned<'_>,
    signer: Option<&Signer>,
) -> Result<Receipt, LedgerError> {
    let (head, prev_hash) = lock_head(conn, "entries").await?;
    let position = head + 1;
    let hash = entry_hash(
        &prev_hash,
        &ChainEntry {
            position,
            entry,
            client: assigned.client,
            idempotency_key: assigned.idempotency_key,
            recorded_at: assigned.recorded_at,
            period: assigned.period,
        },
    );
    let signed = signer.map(|s| {
        (
            s.key_id().to_owned(),
            s.sign(&receipt_message(position, entry.id(), &hash))
                .to_vec(),
        )
    });
    sqlx::query(
        "INSERT INTO entry_chain (position, entry_id, prev_hash, hash, key_id, signature)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(i64::try_from(position).map_err(storage)?)
    .bind(entry.id().as_uuid())
    .bind(&prev_hash[..])
    .bind(&hash[..])
    .bind(signed.as_ref().map(|(k, _)| k.as_str()))
    .bind(signed.as_ref().map(|(_, s)| s.as_slice()))
    .execute(&mut *conn)
    .await
    .map_err(db)?;
    move_head(conn, "entries", position, &hash).await?;
    Ok(Receipt {
        position,
        entry: entry.id(),
        prev_hash,
        hash,
        key_id: signed.as_ref().map(|(k, _)| k.clone()),
        signature: signed.map(|(_, s)| s),
    })
}

fn receipt_from_row(row: &PgRow) -> Result<Receipt, LedgerError> {
    let position: i64 = row.try_get("position").map_err(db)?;
    Ok(Receipt {
        position: u64::try_from(position).map_err(storage)?,
        entry: entry_id(row.try_get("entry_id").map_err(db)?)?,
        prev_hash: hash32(row.try_get("prev_hash").map_err(db)?)?,
        hash: hash32(row.try_get("hash").map_err(db)?)?,
        key_id: row.try_get("key_id").map_err(db)?,
        signature: row.try_get("signature").map_err(db)?,
    })
}

/// An entry's receipt, if it was chained.
pub async fn receipt(
    conn: &mut PgConnection,
    entry: EntryId,
) -> Result<Option<Receipt>, LedgerError> {
    let row = sqlx::query(
        "SELECT position, entry_id, prev_hash, hash, key_id, signature
           FROM entry_chain WHERE entry_id = $1",
    )
    .bind(entry.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(receipt_from_row).transpose()
}

// ------------------------------------------------------------------ audit

/// An audit event to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAuditEvent {
    pub client: Option<ClientId>,
    pub actor: Option<String>,
    /// What was attempted, such as `operations.post` or `token.issue`.
    pub action: String,
    /// What it was done to, such as `entry:<id>`.
    pub target: Option<String>,
    pub request_id: Option<String>,
    /// `ok`, or why not: `forbidden`, `refused`, `failed`...
    pub outcome: String,
    /// Further facts, as JSON text: the status and code, or before and
    /// after for a change of configuration. Never secrets or bodies.
    pub details: String,
}

/// A recorded audit event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub position: u64,
    pub at: Timestamp,
    pub event: NewAuditEvent,
    pub hash: ChainHash,
}

/// Records an audit event, chained, in a transaction of its own.
pub async fn append_audit(
    conn: &mut PgConnection,
    event: &NewAuditEvent,
    at: Timestamp,
) -> Result<AuditEvent, LedgerError> {
    let mut tx = conn.begin().await.map_err(db)?;
    let (head, prev_hash) = lock_head(&mut tx, "audit").await?;
    let position = head + 1;
    let hash = audit_hash(
        &prev_hash,
        &ChainAudit {
            position,
            at,
            client: event.client,
            actor: event.actor.as_deref(),
            action: &event.action,
            target: event.target.as_deref(),
            request_id: event.request_id.as_deref(),
            outcome: &event.outcome,
            details: &event.details,
        },
    );
    sqlx::query(
        "INSERT INTO audit_events
             (position, at, client_id, actor, action, target, request_id, outcome, details,
              prev_hash, hash)
         VALUES ($1, timestamptz 'epoch' + $2::bigint * interval '1 microsecond',
                 $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(i64::try_from(position).map_err(storage)?)
    .bind(at.as_micros())
    .bind(event.client.map(|c| c.as_uuid()))
    .bind(event.actor.as_deref())
    .bind(&event.action)
    .bind(event.target.as_deref())
    .bind(event.request_id.as_deref())
    .bind(&event.outcome)
    .bind(&event.details)
    .bind(&prev_hash[..])
    .bind(&hash[..])
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    move_head(&mut tx, "audit", position, &hash).await?;
    tx.commit().await.map_err(db)?;
    Ok(AuditEvent {
        position,
        at,
        event: event.clone(),
        hash,
    })
}

const AUDIT_COLUMNS: &str = "position, \
     (extract(epoch FROM at) * 1000000)::bigint AS at_micros, client_id, actor, action, target, \
     request_id, outcome, details, prev_hash, hash";

fn audit_from_row(row: &PgRow) -> Result<(AuditEvent, ChainHash), LedgerError> {
    let position: i64 = row.try_get("position").map_err(db)?;
    let client: Option<Uuid> = row.try_get("client_id").map_err(db)?;
    Ok((
        AuditEvent {
            position: u64::try_from(position).map_err(storage)?,
            at: Timestamp::from_micros(row.try_get("at_micros").map_err(db)?),
            event: NewAuditEvent {
                client: client
                    .map(ClientId::from_uuid)
                    .transpose()
                    .map_err(storage)?,
                actor: row.try_get("actor").map_err(db)?,
                action: row.try_get("action").map_err(db)?,
                target: row.try_get("target").map_err(db)?,
                request_id: row.try_get("request_id").map_err(db)?,
                outcome: row.try_get("outcome").map_err(db)?,
                details: row.try_get("details").map_err(db)?,
            },
            hash: hash32(row.try_get("hash").map_err(db)?)?,
        },
        hash32(row.try_get("prev_hash").map_err(db)?)?,
    ))
}

/// Audit events, oldest first, after `after`; all clients, or one.
pub async fn audit_events(
    conn: &mut PgConnection,
    client: Option<ClientId>,
    after: u64,
    limit: i64,
) -> Result<Vec<AuditEvent>, LedgerError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {AUDIT_COLUMNS} FROM audit_events
          WHERE position > $1 AND ($2::uuid IS NULL OR client_id = $2)
          ORDER BY position LIMIT $3"
    )))
    .bind(i64::try_from(after).map_err(storage)?)
    .bind(client.map(|c| c.as_uuid()))
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter()
        .map(|r| audit_from_row(r).map(|(e, _)| e))
        .collect()
}

// ------------------------------------------------------------------ checkpoints

/// The heads of both chains at a period's close, signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub id: i64,
    pub period: i64,
    pub position: u64,
    pub hash: ChainHash,
    pub audit_position: u64,
    pub audit_hash: ChainHash,
    pub at: Timestamp,
    pub key_id: String,
    pub signature: Vec<u8>,
}

impl Checkpoint {
    /// The bytes the signature covers.
    pub fn message(&self) -> Vec<u8> {
        checkpoint_message(
            self.id,
            self.period,
            self.position,
            &self.hash,
            self.audit_position,
            &self.audit_hash,
            self.at,
        )
    }

    /// A one-line text form for files kept outside the database:
    /// `id period position hash audit_position audit_hash at_micros key_id signature`.
    pub fn to_line(&self) -> String {
        format!(
            "mezgeb-checkpoint-v1 {} {} {} {} {} {} {} {} {}",
            self.id,
            self.period,
            self.position,
            hex(&self.hash),
            self.audit_position,
            hex(&self.audit_hash),
            self.at.as_micros(),
            self.key_id,
            hex(&self.signature)
        )
    }

    /// Parses [`Checkpoint::to_line`].
    pub fn from_line(line: &str) -> Option<Checkpoint> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [
            tag,
            id,
            period,
            position,
            hash,
            audit_position,
            audit_hash,
            at,
            key_id,
            signature,
        ] = parts[..]
        else {
            return None;
        };
        if tag != "mezgeb-checkpoint-v1" {
            return None;
        }
        let unhex = |s: &str| -> Option<Vec<u8>> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
                .collect()
        };
        Some(Checkpoint {
            id: id.parse().ok()?,
            period: period.parse().ok()?,
            position: position.parse().ok()?,
            hash: unhex(hash)?.try_into().ok()?,
            audit_position: audit_position.parse().ok()?,
            audit_hash: unhex(audit_hash)?.try_into().ok()?,
            at: Timestamp::from_micros(at.parse().ok()?),
            key_id: key_id.to_owned(),
            signature: unhex(signature)?,
        })
    }
}

const CHECKPOINT_COLUMNS: &str = "id, period_id, position, hash, audit_position, audit_hash, \
     (extract(epoch FROM at) * 1000000)::bigint AS at_micros, key_id, signature";

fn checkpoint_from_row(row: &PgRow) -> Result<Checkpoint, LedgerError> {
    let position: i64 = row.try_get("position").map_err(db)?;
    let audit_position: i64 = row.try_get("audit_position").map_err(db)?;
    Ok(Checkpoint {
        id: row.try_get("id").map_err(db)?,
        period: row.try_get("period_id").map_err(db)?,
        position: u64::try_from(position).map_err(storage)?,
        hash: hash32(row.try_get("hash").map_err(db)?)?,
        audit_position: u64::try_from(audit_position).map_err(storage)?,
        audit_hash: hash32(row.try_get("audit_hash").map_err(db)?)?,
        at: Timestamp::from_micros(row.try_get("at_micros").map_err(db)?),
        key_id: row.try_get("key_id").map_err(db)?,
        signature: row.try_get("signature").map_err(db)?,
    })
}

/// Signs and records a checkpoint of both chains' heads for a closed
/// period. Recording it again for the same period returns the first.
pub async fn record_checkpoint(
    conn: &mut PgConnection,
    period: i64,
    signer: &Signer,
    at: Timestamp,
) -> Result<Checkpoint, LedgerError> {
    let mut tx = conn.begin().await.map_err(db)?;
    let existing = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {CHECKPOINT_COLUMNS} FROM checkpoints WHERE period_id = $1"
    )))
    .bind(period)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    if let Some(row) = existing {
        return checkpoint_from_row(&row);
    }
    signer.register(&mut tx).await?;
    // Both heads locked, so no append slips between reading and signing.
    let (position, hash) = lock_head(&mut tx, "entries").await?;
    let (audit_position, audit_hash) = lock_head(&mut tx, "audit").await?;
    let id: i64 = sqlx::query_scalar("SELECT COALESCE(max(id), 0) + 1 FROM checkpoints")
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
    let mut checkpoint = Checkpoint {
        id,
        period,
        position,
        hash,
        audit_position,
        audit_hash,
        at,
        key_id: signer.key_id().to_owned(),
        signature: Vec::new(),
    };
    checkpoint.signature = signer.sign(&checkpoint.message()).to_vec();
    sqlx::query(
        "INSERT INTO checkpoints
             (id, period_id, position, hash, audit_position, audit_hash, at, key_id, signature)
         VALUES ($1, $2, $3, $4, $5, $6,
                 timestamptz 'epoch' + $7::bigint * interval '1 microsecond', $8, $9)",
    )
    .bind(id)
    .bind(period)
    .bind(i64::try_from(position).map_err(storage)?)
    .bind(&hash[..])
    .bind(i64::try_from(audit_position).map_err(storage)?)
    .bind(&audit_hash[..])
    .bind(at.as_micros())
    .bind(signer.key_id())
    .bind(&checkpoint.signature)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(checkpoint)
}

/// Every checkpoint, oldest first.
pub async fn checkpoints(conn: &mut PgConnection) -> Result<Vec<Checkpoint>, LedgerError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {CHECKPOINT_COLUMNS} FROM checkpoints ORDER BY id"
    )))
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter().map(checkpoint_from_row).collect()
}

/// Every public key, by key ID.
pub async fn signing_keys(
    conn: &mut PgConnection,
) -> Result<BTreeMap<String, Vec<u8>>, LedgerError> {
    let rows = sqlx::query("SELECT key_id, public_key FROM signing_keys ORDER BY added_at, key_id")
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?;
    rows.iter()
        .map(|r| {
            Ok((
                r.try_get("key_id").map_err(db)?,
                r.try_get("public_key").map_err(db)?,
            ))
        })
        .collect()
}

// ------------------------------------------------------------------ verification

/// What chain verification found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChainReport {
    pub entries: u64,
    pub audit_events: u64,
    /// Entries posted before the chain began (before migration 0010).
    pub before_chain: u64,
    pub unsigned: u64,
    pub checkpoints: u64,
    pub findings: Vec<String>,
}

impl ChainReport {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

impl fmt::Display for ChainReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "chain verification: {} entries, {} audit events, {} checkpoints: {}",
            self.entries,
            self.audit_events,
            self.checkpoints,
            if self.is_clean() { "clean" } else { "FINDINGS" }
        )?;
        if self.before_chain > 0 {
            writeln!(
                f,
                "  {} entries were posted before the chain began",
                self.before_chain
            )?;
        }
        if self.unsigned > 0 {
            writeln!(f, "  {} links have no receipt signature", self.unsigned)?;
        }
        for finding in &self.findings {
            writeln!(f, "finding: {finding}")?;
        }
        Ok(())
    }
}

/// Checks a checkpoint's signature, and that both chains pass through its
/// heads: `entries` and `audit` map positions to hashes.
fn check_checkpoint(
    checkpoint: &Checkpoint,
    source: &str,
    keys: &BTreeMap<String, Vec<u8>>,
    entries: &BTreeMap<u64, ChainHash>,
    audit: &BTreeMap<u64, ChainHash>,
    findings: &mut Vec<String>,
) {
    let name = format!("checkpoint {} ({source})", checkpoint.id);
    match keys.get(&checkpoint.key_id) {
        Some(public) if verify_signature(public, &checkpoint.message(), &checkpoint.signature) => {}
        Some(_) => findings.push(format!("{name}: the signature does not verify")),
        None => findings.push(format!("{name}: unknown key {}", checkpoint.key_id)),
    }
    for (chain, links, position, hash) in [
        ("entry", entries, checkpoint.position, checkpoint.hash),
        (
            "audit",
            audit,
            checkpoint.audit_position,
            checkpoint.audit_hash,
        ),
    ] {
        let found = if position == 0 {
            Some(GENESIS)
        } else {
            links.get(&position).copied()
        };
        match found {
            Some(h) if h == hash => {}
            Some(_) => findings.push(format!(
                "{name}: the {chain} chain has another hash at position {position}: history before it was changed"
            )),
            None => findings.push(format!(
                "{name}: the {chain} chain no longer reaches position {position}: links were deleted"
            )),
        }
    }
}

/// Verifies both chains, every receipt and checkpoint signature, and every
/// checkpoint, including `external` copies kept outside the database.
pub async fn verify_chains(
    conn: &mut PgConnection,
    external: &[Checkpoint],
) -> Result<ChainReport, LedgerError> {
    let mut report = ChainReport::default();
    let keys = signing_keys(conn).await?;

    // The entry chain, link by link.
    let rows = sqlx::query(
        "SELECT c.position, c.entry_id, c.prev_hash, c.hash, c.key_id, c.signature,
                e.client_id, e.idempotency_key, e.period_id,
                (extract(epoch FROM e.recorded_time) * 1000000)::bigint AS recorded_micros
           FROM entry_chain c LEFT JOIN journal_entries e ON e.id = c.entry_id
          ORDER BY c.position",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut entries: BTreeMap<u64, ChainHash> = BTreeMap::new();
    let mut previous = GENESIS;
    let mut expected = 1u64;
    for row in &rows {
        let link = receipt_from_row(row)?;
        report.entries += 1;
        let at = format!("entry chain position {}", link.position);
        if link.position != expected {
            report.findings.push(format!(
                "{at}: expected position {expected}: links are missing"
            ));
        }
        expected = link.position + 1;
        if link.prev_hash != previous {
            report
                .findings
                .push(format!("{at}: does not follow the previous link"));
        }
        let client: Option<Uuid> = row.try_get("client_id").map_err(db)?;
        let Some(client) = client else {
            report
                .findings
                .push(format!("{at}: its entry {} is missing", link.entry));
            previous = link.hash;
            entries.insert(link.position, link.hash);
            continue;
        };
        let Some(entry) = load_entry(conn, link.entry).await? else {
            report
                .findings
                .push(format!("{at}: its entry {} is missing", link.entry));
            previous = link.hash;
            continue;
        };
        let key: String = row.try_get("idempotency_key").map_err(db)?;
        let recomputed = entry_hash(
            &link.prev_hash,
            &ChainEntry {
                position: link.position,
                entry: &entry,
                client: ClientId::from_uuid(client).map_err(storage)?,
                idempotency_key: &key,
                recorded_at: Timestamp::from_micros(row.try_get("recorded_micros").map_err(db)?),
                period: row.try_get("period_id").map_err(db)?,
            },
        );
        if recomputed != link.hash {
            report.findings.push(format!(
                "{at}: entry {} does not match its hash: the entry or its lines were changed",
                link.entry
            ));
        }
        match (&link.key_id, &link.signature) {
            (Some(key_id), Some(signature)) => {
                let message = receipt_message(link.position, link.entry, &link.hash);
                if !keys
                    .get(key_id)
                    .is_some_and(|k| verify_signature(k, &message, signature))
                {
                    report
                        .findings
                        .push(format!("{at}: the receipt signature does not verify"));
                }
            }
            _ => report.unsigned += 1,
        }
        previous = link.hash;
        entries.insert(link.position, link.hash);
    }
    let (head_position, head_hash) = head(conn, "entries").await?;
    if head_position != expected - 1 || (head_position > 0 && head_hash != previous) {
        report.findings.push(format!(
            "the entry chain's head (position {head_position}) does not match its last link ({})",
            expected - 1
        ));
    }
    // Every entry posted since the chain began is in it.
    let first_sequence: Option<i64> = sqlx::query_scalar(
        "SELECT min(e.sequence) FROM entry_chain c JOIN journal_entries e ON e.id = c.entry_id",
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    let unchained: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT e.id, e.sequence FROM journal_entries e
          WHERE NOT EXISTS (SELECT 1 FROM entry_chain c WHERE c.entry_id = e.id)
          ORDER BY e.sequence",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    for (id, sequence) in unchained {
        if first_sequence.is_some_and(|first| sequence > first) {
            report.findings.push(format!(
                "entry {id} (sequence {sequence}) is not in the chain"
            ));
        } else {
            report.before_chain += 1;
        }
    }

    // The audit chain.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {AUDIT_COLUMNS} FROM audit_events ORDER BY position"
    )))
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut audit: BTreeMap<u64, ChainHash> = BTreeMap::new();
    let mut previous = GENESIS;
    let mut expected = 1u64;
    for row in &rows {
        let (event, prev_hash) = audit_from_row(row)?;
        report.audit_events += 1;
        let at = format!("audit chain position {}", event.position);
        if event.position != expected {
            report.findings.push(format!(
                "{at}: expected position {expected}: events are missing"
            ));
        }
        expected = event.position + 1;
        if prev_hash != previous {
            report
                .findings
                .push(format!("{at}: does not follow the previous event"));
        }
        let e = &event.event;
        let recomputed = audit_hash(
            &prev_hash,
            &ChainAudit {
                position: event.position,
                at: event.at,
                client: e.client,
                actor: e.actor.as_deref(),
                action: &e.action,
                target: e.target.as_deref(),
                request_id: e.request_id.as_deref(),
                outcome: &e.outcome,
                details: &e.details,
            },
        );
        if recomputed != event.hash {
            report.findings.push(format!("{at}: the event was changed"));
        }
        previous = event.hash;
        audit.insert(event.position, event.hash);
    }
    let (head_position, head_hash) = head(conn, "audit").await?;
    if head_position != expected - 1 || (head_position > 0 && head_hash != previous) {
        report.findings.push(format!(
            "the audit chain's head (position {head_position}) does not match its last event ({})",
            expected - 1
        ));
    }

    // Checkpoints: in the database, and the copies kept elsewhere.
    let stored = checkpoints(conn).await?;
    for checkpoint in &stored {
        report.checkpoints += 1;
        check_checkpoint(
            checkpoint,
            "database",
            &keys,
            &entries,
            &audit,
            &mut report.findings,
        );
    }
    for checkpoint in external {
        report.checkpoints += 1;
        if !stored.iter().any(|s| s == checkpoint) {
            report.findings.push(format!(
                "checkpoint {} (copy): the database no longer holds it as it was",
                checkpoint.id
            ));
        }
        check_checkpoint(
            checkpoint,
            "copy",
            &keys,
            &entries,
            &audit,
            &mut report.findings,
        );
    }
    Ok(report)
}

async fn head(conn: &mut PgConnection, chain: &str) -> Result<(u64, ChainHash), LedgerError> {
    let row = sqlx::query("SELECT position, hash FROM chain_heads WHERE chain = $1")
        .bind(chain)
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
    let position: i64 = row.try_get("position").map_err(db)?;
    Ok((
        u64::try_from(position).map_err(storage)?,
        hash32(row.try_get("hash").map_err(db)?)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_lines_round_trip() {
        let checkpoint = Checkpoint {
            id: 3,
            period: 2,
            position: 41,
            hash: [7; 32],
            audit_position: 9,
            audit_hash: [8; 32],
            at: Timestamp::from_micros(1_794_000_000_000_000),
            key_id: "0123456789abcdef".into(),
            signature: vec![9; 64],
        };
        let line = checkpoint.to_line();
        assert_eq!(Checkpoint::from_line(&line), Some(checkpoint));
        assert_eq!(Checkpoint::from_line("mezgeb-checkpoint-v2 1"), None);
        assert_eq!(Checkpoint::from_line(""), None);
    }
}

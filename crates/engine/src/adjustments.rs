//! Adjustment requests: maker-checker over two calls (step 2.07, INV-25).
//!
//! An adjustment can create money without any payment behind it, so one
//! person requests it and a different person approves it. Over an API those
//! are two calls by two people, and the request waits in
//! `adjustment_requests` in between:
//!
//! ```text
//! pending ──approve──► approving ──posted──► approved
//!    └─────reject────► rejected
//! ```
//!
//! Approving first moves the request to `approving` under a row lock,
//! recording the approver, so it can no longer be rejected; then it posts
//! the adjustment, under an idempotency key derived from the request; then
//! it records the entry. If it stops after the first step, approving again
//! finishes it with the same approver and posts the same entry once.

use std::collections::BTreeMap;

use mezgeb_core::{
    AccountId, ActorId, Authorization, ClientId, EntryDraft, EntryId, EntryKind, IdempotencyKey,
    LedgerError, LineDraft, Money, PostedEntry, PostingError, ReasonCode, Side, Timestamp,
    validate,
};
use sqlx::postgres::PgRow;
use sqlx::{Acquire, PgConnection, Row};
use uuid::Uuid;

use crate::concurrency::PostOptions;
use crate::post::{PostRequest, load_entry, post_entry_with};
use crate::rows::{ACCOUNT_COLUMNS, account_from_row, account_id, db, entry_id, storage};

/// Where an adjustment request is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustmentStatus {
    Pending,
    /// Approved, and being posted.
    Approving,
    Approved,
    Rejected,
}

impl AdjustmentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            AdjustmentStatus::Pending => "pending",
            AdjustmentStatus::Approving => "approving",
            AdjustmentStatus::Approved => "approved",
            AdjustmentStatus::Rejected => "rejected",
        }
    }

    fn from_name(name: &str) -> Option<AdjustmentStatus> {
        [
            AdjustmentStatus::Pending,
            AdjustmentStatus::Approving,
            AdjustmentStatus::Approved,
            AdjustmentStatus::Rejected,
        ]
        .into_iter()
        .find(|s| s.as_str() == name)
    }
}

/// One adjustment request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjustmentRequest {
    pub id: Uuid,
    pub client: ClientId,
    pub status: AdjustmentStatus,
    pub reason: ReasonCode,
    pub lines: Vec<LineDraft>,
    pub metadata: BTreeMap<String, String>,
    pub note: Option<String>,
    pub requested_by: ActorId,
    pub requested_at: Timestamp,
    pub decided_by: Option<ActorId>,
    pub decided_at: Option<Timestamp>,
    pub decision_note: Option<String>,
    pub entry: Option<EntryId>,
}

/// What a request or decision did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<T> {
    /// This call did it.
    Done(T),
    /// An earlier call already did it; this is its result.
    Repeated(T),
}

impl<T> Outcome<T> {
    pub fn value(&self) -> &T {
        match self {
            Outcome::Done(v) | Outcome::Repeated(v) => v,
        }
    }

    pub fn into_value(self) -> T {
        match self {
            Outcome::Done(v) | Outcome::Repeated(v) => v,
        }
    }
}

const COLUMNS: &str = "id, client_id, status, reason_code, lines::text AS lines, \
     metadata::text AS metadata, note, requested_by, \
     (extract(epoch FROM requested_at) * 1000000)::bigint AS requested_micros, decided_by, \
     (extract(epoch FROM decided_at) * 1000000)::bigint AS decided_micros, decision_note, entry_id";

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredLine {
    account: Uuid,
    side: String,
    amount: i64,
    currency: String,
}

fn lines_to_json(lines: &[LineDraft]) -> Result<String, LedgerError> {
    let stored: Vec<StoredLine> = lines
        .iter()
        .map(|l| StoredLine {
            account: l.account.as_uuid(),
            side: l.side.as_str().to_owned(),
            amount: l.amount.amount(),
            currency: l.amount.currency().code().to_owned(),
        })
        .collect();
    serde_json::to_string(&stored).map_err(storage)
}

fn request_from_row(row: &PgRow) -> Result<AdjustmentRequest, LedgerError> {
    let status: String = row.try_get("status").map_err(db)?;
    let lines: String = row.try_get("lines").map_err(db)?;
    let stored: Vec<StoredLine> = serde_json::from_str(&lines).map_err(storage)?;
    let mut drafts = Vec::with_capacity(stored.len());
    for line in stored {
        let currency = mezgeb_core::Currency::from_code(&line.currency).map_err(storage)?;
        drafts.push(LineDraft {
            account: account_id(line.account)?,
            side: Side::from_name(&line.side).ok_or_else(|| storage("stored side is invalid"))?,
            amount: Money::from_minor(line.amount, currency),
        });
    }
    let metadata: String = row.try_get("metadata").map_err(db)?;
    let decided_by: Option<String> = row.try_get("decided_by").map_err(db)?;
    let decided: Option<i64> = row.try_get("decided_micros").map_err(db)?;
    let entry: Option<Uuid> = row.try_get("entry_id").map_err(db)?;
    Ok(AdjustmentRequest {
        id: row.try_get("id").map_err(db)?,
        client: ClientId::from_uuid(row.try_get("client_id").map_err(db)?).map_err(storage)?,
        status: AdjustmentStatus::from_name(&status)
            .ok_or_else(|| storage("stored adjustment status is invalid"))?,
        reason: ReasonCode::new(row.try_get::<String, _>("reason_code").map_err(db)?)
            .map_err(storage)?,
        lines: drafts,
        metadata: serde_json::from_str(&metadata).map_err(storage)?,
        note: row.try_get("note").map_err(db)?,
        requested_by: ActorId::new(row.try_get::<String, _>("requested_by").map_err(db)?)
            .map_err(storage)?,
        requested_at: Timestamp::from_micros(row.try_get("requested_micros").map_err(db)?),
        decided_by: decided_by.map(ActorId::new).transpose().map_err(storage)?,
        decided_at: decided.map(Timestamp::from_micros),
        decision_note: row.try_get("decision_note").map_err(db)?,
        entry: entry.map(entry_id).transpose()?,
    })
}

async fn load(
    conn: &mut PgConnection,
    condition: &str,
    id: Uuid,
) -> Result<Option<AdjustmentRequest>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM adjustment_requests WHERE id = $1{condition}"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(request_from_row).transpose()
}

/// An adjustment request, by ID.
pub async fn get_adjustment(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<AdjustmentRequest>, LedgerError> {
    load(conn, "", id).await
}

/// What a new adjustment request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdjustment {
    pub reason: ReasonCode,
    pub lines: Vec<LineDraft>,
    pub metadata: BTreeMap<String, String>,
    pub note: Option<String>,
    pub requested_by: ActorId,
}

impl NewAdjustment {
    fn draft(&self) -> EntryDraft {
        EntryDraft {
            lines: self.lines.clone(),
            metadata: self.metadata.clone(),
            ..EntryDraft::default()
        }
    }
}

/// Files an adjustment request, once per `(client, key)`. The lines are
/// checked now against every posting rule except the balances, which are
/// checked again, with the accounts locked, when it is approved.
pub async fn request_adjustment(
    conn: &mut PgConnection,
    id: Uuid,
    client: ClientId,
    key: &IdempotencyKey,
    request: &NewAdjustment,
) -> Result<Outcome<AdjustmentRequest>, LedgerError> {
    if request.reason.kind() != Some(EntryKind::Adjustment) {
        return Err(LedgerError::ReasonNotAllowed(
            request.reason.as_str().to_owned(),
        ));
    }
    let ids: Vec<Uuid> = request.lines.iter().map(|l| l.account.as_uuid()).collect();
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ANY($1)"
    )))
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut accounts: BTreeMap<AccountId, mezgeb_core::Account> = BTreeMap::new();
    for row in &rows {
        let account = account_from_row(row)?;
        accounts.insert(account.id(), account);
    }
    match validate(request.draft(), &accounts) {
        Ok(_)
        | Err(PostingError::InsufficientFunds { .. })
        | Err(PostingError::AccountTotalOverflow { .. }) => {}
        Err(error) => return Err(LedgerError::Posting(error)),
    }

    let lines = lines_to_json(&request.lines)?;
    let metadata = serde_json::to_string(&request.metadata).map_err(storage)?;
    sqlx::query(
        "INSERT INTO adjustment_requests
             (id, client_id, idempotency_key, status, reason_code, lines, metadata, note, requested_by)
         VALUES ($1, $2, $3, 'pending', $4, $5::jsonb, $6::jsonb, $7, $8)
         ON CONFLICT (client_id, idempotency_key) DO NOTHING",
    )
    .bind(id)
    .bind(client.as_uuid())
    .bind(key.as_str())
    .bind(request.reason.as_str())
    .bind(&lines)
    .bind(&metadata)
    .bind(request.note.as_deref())
    .bind(request.requested_by.as_str())
    .execute(&mut *conn)
    .await
    .map_err(db)?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM adjustment_requests WHERE client_id = $1 AND idempotency_key = $2"
    )))
    .bind(client.as_uuid())
    .bind(key.as_str())
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    let stored = request_from_row(&row)?;
    if stored.id == id {
        return Ok(Outcome::Done(stored));
    }
    let same = stored.reason == request.reason
        && stored.lines == request.lines
        && stored.metadata == request.metadata
        && stored.note == request.note
        && stored.requested_by == request.requested_by;
    if same {
        Ok(Outcome::Repeated(stored))
    } else {
        Err(LedgerError::IdempotencyKeyReused)
    }
}

/// Approves a pending request as `approver` and posts the adjustment.
/// Repeating the approval of an approved request returns its entry. If the
/// ledger refuses the lines, nothing is posted and the request is pending
/// again.
pub async fn approve_adjustment(
    conn: &mut PgConnection,
    id: Uuid,
    approver: &ActorId,
    options: &PostOptions,
) -> Result<Outcome<(AdjustmentRequest, PostedEntry)>, LedgerError> {
    // 1. pending -> approving, under the row lock.
    let mut tx = conn.begin().await.map_err(db)?;
    let request = load(&mut tx, " FOR UPDATE", id)
        .await?
        .ok_or(LedgerError::AdjustmentNotFound)?;
    let approver = match request.status {
        AdjustmentStatus::Rejected => return Err(LedgerError::AdjustmentNotPending),
        AdjustmentStatus::Approved => {
            let entry = request
                .entry
                .ok_or_else(|| storage("approved request has no entry"))?;
            drop(tx);
            let posted = load_entry(conn, entry)
                .await?
                .ok_or_else(|| storage("approved request's entry is missing"))?;
            return Ok(Outcome::Repeated((request, posted)));
        }
        AdjustmentStatus::Approving => request
            .decided_by
            .clone()
            .ok_or_else(|| storage("approving request has no approver"))?,
        AdjustmentStatus::Pending => {
            if *approver == request.requested_by {
                return Err(LedgerError::MakerIsChecker);
            }
            sqlx::query(
                "UPDATE adjustment_requests SET status = 'approving', decided_by = $2 WHERE id = $1",
            )
            .bind(id)
            .bind(approver.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            approver.clone()
        }
    };
    tx.commit().await.map_err(db)?;

    // 2. Post the adjustment, idempotently.
    let key = IdempotencyKey::new(format!("adjustment-{id}")).map_err(storage)?;
    let entry_id = EntryId::from_uuid(Uuid::now_v7()).map_err(storage)?;
    let authorization = Authorization {
        reason: request.reason.clone(),
        requested_by: request.requested_by.clone(),
        approved_by: Some(approver),
    };
    let draft = EntryDraft {
        lines: request.lines.clone(),
        metadata: request.metadata.clone(),
        ..EntryDraft::default()
    };
    let posted = match post_entry_with(
        conn,
        PostRequest::new(entry_id, request.client, key, draft).authorized(authorization),
        options,
    )
    .await
    {
        Ok(posted) => posted,
        // The ledger refused the lines (an account would go negative, or is
        // no longer open): nothing was posted. The request goes back to
        // pending, so a checker can approve it later or reject it.
        Err(refused @ LedgerError::Posting(_)) => {
            sqlx::query(
                "UPDATE adjustment_requests SET status = 'pending', decided_by = NULL
                  WHERE id = $1 AND status = 'approving'",
            )
            .bind(id)
            .execute(&mut *conn)
            .await
            .map_err(db)?;
            return Err(refused);
        }
        Err(other) => return Err(other),
    };

    // 3. approving -> approved.
    sqlx::query(
        "UPDATE adjustment_requests
            SET status = 'approved', decided_at = clock_timestamp(), entry_id = $2
          WHERE id = $1 AND status = 'approving'",
    )
    .bind(id)
    .bind(posted.id().as_uuid())
    .execute(&mut *conn)
    .await
    .map_err(db)?;
    let request = load(conn, "", id)
        .await?
        .ok_or(LedgerError::AdjustmentNotFound)?;
    Ok(Outcome::Done((request, posted)))
}

/// Rejects a pending request as `checker`. Nothing is posted.
pub async fn reject_adjustment(
    conn: &mut PgConnection,
    id: Uuid,
    checker: &ActorId,
    note: &str,
) -> Result<Outcome<AdjustmentRequest>, LedgerError> {
    let mut tx = conn.begin().await.map_err(db)?;
    let request = load(&mut tx, " FOR UPDATE", id)
        .await?
        .ok_or(LedgerError::AdjustmentNotFound)?;
    match request.status {
        AdjustmentStatus::Rejected if request.decided_by.as_ref() == Some(checker) => {
            return Ok(Outcome::Repeated(request));
        }
        AdjustmentStatus::Pending => {}
        _ => return Err(LedgerError::AdjustmentNotPending),
    }
    if *checker == request.requested_by {
        return Err(LedgerError::MakerIsChecker);
    }
    sqlx::query(
        "UPDATE adjustment_requests
            SET status = 'rejected', decided_by = $2, decided_at = clock_timestamp(),
                decision_note = $3
          WHERE id = $1",
    )
    .bind(id)
    .bind(checker.as_str())
    .bind(note)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    let request = load(&mut tx, "", id)
        .await?
        .ok_or(LedgerError::AdjustmentNotFound)?;
    tx.commit().await.map_err(db)?;
    Ok(Outcome::Done(request))
}

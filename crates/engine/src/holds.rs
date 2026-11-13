//! Holds on PostgreSQL (step 1.14, ADR-0004).
//!
//! The same operations as the reference ledger: place, capture (fully or
//! partly, the rest returned in the same entry), release and expire. Each is
//! one transaction, idempotent like any posting, and locks the hold's row,
//! so a capture racing an expiry or a release has exactly one winner
//! (INV-16). The database checks the hold's rules again (migration 0005).

use std::collections::{BTreeMap, BTreeSet};

use mezgeb_core::{
    AccountId, AccountType, ClientId, Currency, EntryDraft, EntryId, Fingerprint, Hold, HoldId,
    HoldStatus, IdempotencyKey, LedgerError, LineDraft, Money, PositiveAmount, Side, Timestamp,
    hold_capture_fingerprint, hold_end_fingerprint, hold_place_fingerprint,
};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::concurrency::PostOptions;
use crate::post::{Claim, PostRequest, claim_key, entry_exists, post_in_transaction};
use crate::rows::{account_id, constraint_violation, db, entry_id, storage};

/// Who is asking, under which idempotency key, and the entry's ID.
#[derive(Debug, Clone)]
pub struct HoldRequest {
    pub entry_id: EntryId,
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
}

/// What to place.
#[derive(Debug, Clone, Copy)]
pub struct PlaceHold {
    pub hold: HoldId,
    pub account: AccountId,
    pub held: AccountId,
    pub amount: PositiveAmount,
    pub expires_at: Timestamp,
}

/// What to capture.
#[derive(Debug, Clone, Copy)]
pub struct CaptureHold {
    pub hold: HoldId,
    pub destination: AccountId,
    pub amount: PositiveAmount,
    /// The current time, from the caller's clock.
    pub now: Timestamp,
}

const HOLD_COLUMNS: &str = "id, account_id, held_account_id, amount, currency, captured, status, \
     (extract(epoch FROM expires_at) * 1000000)::bigint AS expires_micros, \
     place_entry_id, close_entry_id";

fn hold_from_row(row: &PgRow) -> Result<Hold, LedgerError> {
    let id: Uuid = row.try_get("id").map_err(db)?;
    let currency: String = row.try_get("currency").map_err(db)?;
    let currency = Currency::from_code(&currency).map_err(storage)?;
    let amount: i64 = row.try_get("amount").map_err(db)?;
    let status: String = row.try_get("status").map_err(db)?;
    let close: Option<Uuid> = row.try_get("close_entry_id").map_err(db)?;
    Ok(Hold::new(
        HoldId::from_uuid(id).map_err(storage)?,
        account_id(row.try_get("account_id").map_err(db)?)?,
        account_id(row.try_get("held_account_id").map_err(db)?)?,
        PositiveAmount::new(Money::from_minor(amount, currency)).map_err(storage)?,
        row.try_get("captured").map_err(db)?,
        HoldStatus::from_name(&status).ok_or_else(|| storage("stored hold status is invalid"))?,
        Timestamp::from_micros(row.try_get("expires_micros").map_err(db)?),
        entry_id(row.try_get("place_entry_id").map_err(db)?)?,
        close.map(entry_id).transpose()?,
    ))
}

/// Reads one hold, locking its row if `lock` is set.
async fn load_hold(
    conn: &mut PgConnection,
    id: HoldId,
    lock: bool,
) -> Result<Option<Hold>, LedgerError> {
    let lock = if lock { " FOR UPDATE" } else { "" };
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {HOLD_COLUMNS} FROM holds WHERE id = $1{lock}"
    )))
    .bind(id.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(hold_from_row).transpose()
}

/// The hold an entry placed or ended, for answering a repeated request.
async fn hold_of_entry(conn: &mut PgConnection, entry: EntryId) -> Result<Hold, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {HOLD_COLUMNS} FROM holds WHERE place_entry_id = $1 OR close_entry_id = $1"
    )))
    .bind(entry.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?
    .ok_or_else(|| storage("an idempotency key points to an entry with no hold"))?;
    hold_from_row(&row)
}

/// Every hold, ordered by ID.
pub async fn all_holds(conn: &mut PgConnection) -> Result<Vec<Hold>, LedgerError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {HOLD_COLUMNS} FROM holds ORDER BY id"
    )))
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter().map(hold_from_row).collect()
}

/// Refuses ordinary postings to a hold's held account (INV-18): the first
/// such account, in the order given, is reported.
pub(crate) async fn refuse_held(
    conn: &mut PgConnection,
    accounts: &[AccountId],
) -> Result<(), LedgerError> {
    let ids: Vec<Uuid> = accounts.iter().map(|a| a.as_uuid()).collect();
    let held: BTreeSet<Uuid> =
        sqlx::query_scalar("SELECT held_account_id FROM holds WHERE held_account_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?
            .into_iter()
            .collect();
    match accounts.iter().find(|a| held.contains(&a.as_uuid())) {
        Some(account) => Err(LedgerError::AccountIsHeld(*account)),
        None => Ok(()),
    }
}

/// Each account's type and totals, if it exists.
async fn account_facts(
    conn: &mut PgConnection,
    ids: &[AccountId],
) -> Result<BTreeMap<AccountId, (AccountType, i64, i64)>, LedgerError> {
    let uuids: Vec<Uuid> = ids.iter().map(|a| a.as_uuid()).collect();
    let rows = sqlx::query(
        "SELECT id, account_type, debits_total, credits_total FROM accounts WHERE id = ANY($1)",
    )
    .bind(&uuids)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut facts = BTreeMap::new();
    for row in rows {
        let kind: String = row.try_get("account_type").map_err(db)?;
        facts.insert(
            account_id(row.try_get("id").map_err(db)?)?,
            (
                AccountType::from_name(&kind).ok_or_else(|| storage("bad account type"))?,
                row.try_get("debits_total").map_err(db)?,
                row.try_get("credits_total").map_err(db)?,
            ),
        );
    }
    Ok(facts)
}

const fn opposite(side: Side) -> Side {
    match side {
        Side::Debit => Side::Credit,
        Side::Credit => Side::Debit,
    }
}

/// Posts one of a hold's own entries inside the transaction, tagged with the
/// hold and the action.
async fn post_hold_entry(
    conn: &mut PgConnection,
    request: &HoldRequest,
    hold: HoldId,
    action: &str,
    lines: Vec<LineDraft>,
    options: &PostOptions,
) -> Result<(), LedgerError> {
    let draft = EntryDraft {
        lines,
        metadata: BTreeMap::from([
            ("hold".to_owned(), hold.to_string()),
            ("hold_action".to_owned(), action.to_owned()),
        ]),
        allow_duplicate_lines: true,
        ..EntryDraft::default()
    };
    let posting = PostRequest::new(
        request.entry_id,
        request.client_id,
        request.idempotency_key.clone(),
        draft,
    );
    post_in_transaction(conn, posting, None, options).await?;
    Ok(())
}

/// Starts a hold operation: a transaction, the duplicate-entry check and the
/// idempotency claim. `Ok(Err(hold))` means the same request already
/// happened, and `hold` is the answer.
async fn begin<'c>(
    conn: &'c mut PgConnection,
    request: &HoldRequest,
    fingerprint: &Fingerprint,
    options: &PostOptions,
) -> Result<Result<sqlx::Transaction<'c, sqlx::Postgres>, Hold>, LedgerError> {
    let mut tx = options.begin(conn).await?;
    if entry_exists(&mut tx, request.entry_id).await? {
        return Err(LedgerError::DuplicateEntry(request.entry_id));
    }
    let claim = PostRequest::new(
        request.entry_id,
        request.client_id,
        request.idempotency_key.clone(),
        EntryDraft::default(),
    );
    match claim_key(&mut tx, &claim, fingerprint).await? {
        Claim::Existing(entry) => Ok(Err(hold_of_entry(&mut tx, entry.id()).await?)),
        Claim::New => Ok(Ok(tx)),
    }
}

async fn place_once(
    conn: &mut PgConnection,
    request: &HoldRequest,
    place: PlaceHold,
    options: &PostOptions,
) -> Result<Hold, LedgerError> {
    let fingerprint = hold_place_fingerprint(
        place.hold,
        place.account,
        place.held,
        place.amount,
        place.expires_at,
    );
    let mut tx = match begin(conn, request, &fingerprint, options).await? {
        Ok(tx) => tx,
        Err(existing) => return Ok(existing),
    };
    if load_hold(&mut tx, place.hold, false).await?.is_some() {
        return Err(LedgerError::DuplicateHold(place.hold));
    }
    refuse_held(&mut tx, &[place.account]).await?;
    let facts = account_facts(&mut tx, &[place.account, place.held]).await?;
    let (source_type, ..) = *facts
        .get(&place.account)
        .ok_or(LedgerError::AccountNotFound(place.account))?;
    let (held_type, held_debits, held_credits) = *facts
        .get(&place.held)
        .ok_or(LedgerError::AccountNotFound(place.held))?;
    let used: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM holds WHERE held_account_id = $1)")
            .bind(place.held.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
    let usable = place.held != place.account
        && !used
        && held_debits == 0
        && held_credits == 0
        && held_type == source_type;
    if !usable {
        return Err(LedgerError::InvalidHeldAccount(place.held));
    }

    let side = source_type.normal_side();
    let money = place.amount.money();
    post_hold_entry(
        &mut tx,
        request,
        place.hold,
        "place",
        vec![
            LineDraft {
                account: place.account,
                side: opposite(side),
                amount: money,
            },
            LineDraft {
                account: place.held,
                side,
                amount: money,
            },
        ],
        options,
    )
    .await?;
    sqlx::query(
        "INSERT INTO holds
             (id, account_id, held_account_id, amount, currency, expires_at, place_entry_id)
         VALUES ($1, $2, $3, $4, $5,
                 timestamptz 'epoch' + $6::bigint * interval '1 microsecond', $7)",
    )
    .bind(place.hold.as_uuid())
    .bind(place.account.as_uuid())
    .bind(place.held.as_uuid())
    .bind(place.amount.amount())
    .bind(place.amount.currency().code())
    .bind(place.expires_at.as_micros())
    .bind(request.entry_id.as_uuid())
    .execute(&mut *tx)
    .await
    .map_err(|e| match constraint_violation(&e) {
        // Two placements raced for the same held account.
        Some((code, constraint))
            if code == "23505" && constraint == "holds_held_account_id_key" =>
        {
            LedgerError::InvalidHeldAccount(place.held)
        }
        _ => db(e),
    })?;
    let hold = load_hold(&mut tx, place.hold, false)
        .await?
        .ok_or(LedgerError::HoldNotFound(place.hold))?;
    tx.commit().await.map_err(db)?;
    Ok(hold)
}

/// Locks an active hold, or says why it cannot be used.
async fn lock_active(conn: &mut PgConnection, id: HoldId) -> Result<Hold, LedgerError> {
    let hold = load_hold(conn, id, true)
        .await?
        .ok_or(LedgerError::HoldNotFound(id))?;
    if hold.status() != HoldStatus::Active {
        return Err(LedgerError::HoldNotActive {
            hold: id,
            status: hold.status(),
        });
    }
    Ok(hold)
}

async fn held_side(conn: &mut PgConnection, hold: &Hold) -> Result<Side, LedgerError> {
    let facts = account_facts(conn, &[hold.held_account()]).await?;
    facts
        .get(&hold.held_account())
        .map(|(kind, ..)| kind.normal_side())
        .ok_or(LedgerError::AccountNotFound(hold.held_account()))
}

async fn close(
    conn: &mut PgConnection,
    hold: HoldId,
    status: HoldStatus,
    captured: i64,
    entry: EntryId,
) -> Result<Hold, LedgerError> {
    sqlx::query("UPDATE holds SET status = $2, captured = $3, close_entry_id = $4 WHERE id = $1")
        .bind(hold.as_uuid())
        .bind(status.as_str())
        .bind(captured)
        .bind(entry.as_uuid())
        .execute(&mut *conn)
        .await
        .map_err(db)?;
    load_hold(conn, hold, false)
        .await?
        .ok_or(LedgerError::HoldNotFound(hold))
}

async fn capture_once(
    conn: &mut PgConnection,
    request: &HoldRequest,
    capture: CaptureHold,
    options: &PostOptions,
) -> Result<Hold, LedgerError> {
    let fingerprint = hold_capture_fingerprint(capture.hold, capture.destination, capture.amount);
    let mut tx = match begin(conn, request, &fingerprint, options).await? {
        Ok(tx) => tx,
        Err(existing) => return Ok(existing),
    };
    let hold = lock_active(&mut tx, capture.hold).await?;
    if capture.now >= hold.expires_at() {
        return Err(LedgerError::HoldExpired(capture.hold));
    }
    if capture.amount.amount() > hold.amount().amount() {
        return Err(LedgerError::CaptureExceedsHold(capture.hold));
    }
    refuse_held(&mut tx, &[capture.destination]).await?;
    let side = held_side(&mut tx, &hold).await?;
    let total = hold.amount().money();
    let remainder = hold.amount().amount() - capture.amount.amount();
    let mut lines = vec![
        LineDraft {
            account: hold.held_account(),
            side: opposite(side),
            amount: total,
        },
        LineDraft {
            account: capture.destination,
            side,
            amount: capture.amount.money(),
        },
    ];
    if remainder > 0 {
        lines.push(LineDraft {
            account: hold.account(),
            side,
            amount: Money::from_minor(remainder, total.currency()),
        });
    }
    post_hold_entry(&mut tx, request, capture.hold, "capture", lines, options).await?;
    let closed = close(
        &mut tx,
        capture.hold,
        HoldStatus::Captured,
        capture.amount.amount(),
        request.entry_id,
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(closed)
}

async fn end_once(
    conn: &mut PgConnection,
    request: &HoldRequest,
    hold_id: HoldId,
    expire_at: Option<Timestamp>,
    options: &PostOptions,
) -> Result<Hold, LedgerError> {
    let fingerprint = hold_end_fingerprint(hold_id, expire_at.is_some());
    let mut tx = match begin(conn, request, &fingerprint, options).await? {
        Ok(tx) => tx,
        Err(existing) => return Ok(existing),
    };
    let hold = lock_active(&mut tx, hold_id).await?;
    if let Some(now) = expire_at
        && now < hold.expires_at()
    {
        return Err(LedgerError::HoldNotExpired(hold_id));
    }
    let side = held_side(&mut tx, &hold).await?;
    let money = hold.amount().money();
    let (action, status) = match expire_at {
        Some(_) => ("expire", HoldStatus::Expired),
        None => ("release", HoldStatus::Released),
    };
    post_hold_entry(
        &mut tx,
        request,
        hold_id,
        action,
        vec![
            LineDraft {
                account: hold.held_account(),
                side: opposite(side),
                amount: money,
            },
            LineDraft {
                account: hold.account(),
                side,
                amount: money,
            },
        ],
        options,
    )
    .await?;
    let closed = close(&mut tx, hold_id, status, 0, request.entry_id).await?;
    tx.commit().await.map_err(db)?;
    Ok(closed)
}

/// Re-runs a hold operation after a deadlock or serialization failure.
macro_rules! with_retries {
    ($options:expr, $attempt:expr) => {{
        let mut attempt = 1;
        loop {
            match $attempt.await {
                Err(error) if $options.retry_after(&error, attempt).await => attempt += 1,
                other => break other,
            }
        }
    }};
}

/// Places a hold (E02, E16).
pub async fn place_hold(
    conn: &mut PgConnection,
    request: &HoldRequest,
    place: PlaceHold,
    options: &PostOptions,
) -> Result<Hold, LedgerError> {
    with_retries!(options, place_once(conn, request, place, options))
}

/// Captures part or all of an active hold (E03, E17).
pub async fn capture_hold(
    conn: &mut PgConnection,
    request: &HoldRequest,
    capture: CaptureHold,
    options: &PostOptions,
) -> Result<Hold, LedgerError> {
    with_retries!(options, capture_once(conn, request, capture, options))
}

/// Releases an active hold (E04).
pub async fn release_hold(
    conn: &mut PgConnection,
    request: &HoldRequest,
    hold: HoldId,
    options: &PostOptions,
) -> Result<Hold, LedgerError> {
    with_retries!(options, end_once(conn, request, hold, None, options))
}

/// Expires an active hold whose expiry time has passed (E18).
pub async fn expire_hold(
    conn: &mut PgConnection,
    request: &HoldRequest,
    hold: HoldId,
    now: Timestamp,
    options: &PostOptions,
) -> Result<Hold, LedgerError> {
    with_retries!(options, end_once(conn, request, hold, Some(now), options))
}

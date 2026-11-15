//! Posted entries as the API returns them (step 2.07): with when they were
//! recorded and which period they belong to.

use mezgeb_core::{ClientId, EntryId, IdempotencyKey, LedgerError, PostedEntry, Timestamp};
use sqlx::{PgConnection, Row};

use crate::post::{load_entry, lookup_by_idempotency_key};
use crate::rows::db;

/// A posted entry with its recording time and period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRecord {
    pub entry: PostedEntry,
    /// When it was recorded: its posting date.
    pub recorded_at: Timestamp,
    /// Its accounting period; `None` for an entry posted before periods
    /// existed.
    pub period_id: Option<i64>,
    /// The client that posted it.
    pub client: ClientId,
    /// Its signed receipt (INV-26); `None` before the chain began.
    pub receipt: Option<crate::tamper::Receipt>,
}

/// An entry, by ID.
pub async fn entry_record(
    conn: &mut PgConnection,
    id: EntryId,
) -> Result<Option<EntryRecord>, LedgerError> {
    let Some(entry) = load_entry(conn, id).await? else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT (extract(epoch FROM recorded_time) * 1000000)::bigint AS recorded_micros,
                period_id, client_id
           FROM journal_entries WHERE id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    let receipt = crate::tamper::receipt(conn, id).await?;
    Ok(Some(EntryRecord {
        receipt,
        entry,
        recorded_at: Timestamp::from_micros(row.try_get("recorded_micros").map_err(db)?),
        period_id: row.try_get("period_id").map_err(db)?,
        client: ClientId::from_uuid(row.try_get("client_id").map_err(db)?)
            .map_err(crate::rows::storage)?,
    }))
}

/// The entry `client` posted with `key`, if any.
pub async fn entry_record_by_key(
    conn: &mut PgConnection,
    client: ClientId,
    key: &IdempotencyKey,
) -> Result<Option<EntryRecord>, LedgerError> {
    match lookup_by_idempotency_key(conn, client, key).await? {
        Some(entry) => entry_record(conn, entry.id()).await,
        None => Ok(None),
    }
}

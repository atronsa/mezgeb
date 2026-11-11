//! `PgStore`: the [`Ledger`] operations on PostgreSQL.

use std::collections::BTreeMap;

use mezgeb_core::{
    Account, AccountId, AccountStatus, Chart, ClientId, EntryDraft, EntryId, IdempotencyKey,
    Ledger, LedgerError, PostedEntry, PostedLine,
};
use sqlx::{Acquire, PgPool, Row};

use crate::post::{PostRequest, post_entry, reverse_entry};
use crate::rows::{
    ACCOUNT_COLUMNS, account_from_row, constraint_violation, entry_from_row, entry_id,
    line_from_row, storage,
};

/// A ledger stored in PostgreSQL, posting on behalf of one client.
///
/// Connect it as `mezgeb_app` (docs/db-schema.md#roles).
#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
    client: ClientId,
}

impl PgStore {
    pub fn new(pool: PgPool, client: ClientId) -> PgStore {
        PgStore { pool, client }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The idempotency key used for an entry posted through the `Ledger`
    /// trait, which has no key of its own. Step 1.11 adds client keys.
    fn key_for(id: EntryId) -> Result<IdempotencyKey, LedgerError> {
        IdempotencyKey::new(format!("entry-{id}")).map_err(storage)
    }
}

impl Ledger for PgStore {
    async fn open_account(
        &mut self,
        id: AccountId,
        chart: &Chart,
        reference: &str,
    ) -> Result<Account, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1)")
                .bind(id.as_uuid())
                .fetch_one(&mut *conn)
                .await
                .map_err(storage)?;
        if exists {
            return Err(LedgerError::DuplicateAccount(id));
        }
        let account = Account::open(id, chart, reference).map_err(LedgerError::Chart)?;
        sqlx::query(
            "INSERT INTO accounts
                 (id, code, sub_account, name, account_type, currency, status, allow_negative)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id.as_uuid())
        .bind(i16::try_from(account.code().value()).map_err(storage)?)
        .bind(account.sub_account())
        .bind(account.name())
        .bind(account.account_type().as_str())
        .bind(account.currency().code())
        .bind(account.status().as_str())
        .bind(account.allow_negative())
        .execute(&mut *conn)
        .await
        .map_err(|e| match constraint_violation(&e) {
            Some((code, constraint))
                if code == "23505" && constraint == "accounts_reference_unique" =>
            {
                LedgerError::DuplicateReference(account.reference())
            }
            Some((code, constraint)) if code == "23505" && constraint == "accounts_pkey" => {
                LedgerError::DuplicateAccount(id)
            }
            _ => storage(e),
        })?;
        Ok(account)
    }

    async fn set_status(&mut self, id: AccountId, to: AccountStatus) -> Result<(), LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        let mut tx = conn.begin().await.map_err(storage)?;
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1 FOR UPDATE"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or(LedgerError::AccountNotFound(id))?;
        let mut account = account_from_row(&row)?;
        account.set_status(to).map_err(LedgerError::Status)?;
        sqlx::query("UPDATE accounts SET status = $2, version = version + 1 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(to.as_str())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        tx.commit().await.map_err(storage)
    }

    async fn post(&mut self, id: EntryId, draft: EntryDraft) -> Result<PostedEntry, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        let request = PostRequest {
            entry_id: id,
            client_id: self.client,
            idempotency_key: PgStore::key_for(id)?,
            draft,
            reverses: None,
        };
        post_entry(&mut conn, request).await
    }

    async fn reverse(
        &mut self,
        original: EntryId,
        id: EntryId,
    ) -> Result<PostedEntry, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        let request = PostRequest {
            entry_id: id,
            client_id: self.client,
            idempotency_key: PgStore::key_for(id)?,
            draft: EntryDraft::default(),
            reverses: Some(original),
        };
        reverse_entry(&mut conn, original, request).await
    }

    async fn get_account(&mut self, id: AccountId) -> Result<Option<Account>, LedgerError> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.as_ref().map(account_from_row).transpose()
    }

    async fn all_accounts(&mut self) -> Result<Vec<Account>, LedgerError> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY id"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.iter().map(account_from_row).collect()
    }

    async fn all_entries(&mut self) -> Result<Vec<PostedEntry>, LedgerError> {
        let line_rows = sqlx::query(
            "SELECT entry_id, account_id, side, amount, currency, debits_after, credits_after
               FROM journal_lines ORDER BY entry_id, line_no",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        let mut lines: BTreeMap<EntryId, Vec<PostedLine>> = BTreeMap::new();
        for row in &line_rows {
            let entry = entry_id(row.try_get("entry_id").map_err(storage)?)?;
            lines.entry(entry).or_default().push(line_from_row(row)?);
        }
        let entry_rows = sqlx::query(
            "SELECT id, sequence, reverses_entry_id, metadata::text AS metadata,
                    (extract(epoch FROM event_time) * 1000000)::bigint AS event_time_micros
               FROM journal_entries ORDER BY sequence",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        entry_rows
            .iter()
            .map(|row| {
                let id = entry_id(row.try_get("id").map_err(storage)?)?;
                entry_from_row(row, lines.remove(&id).unwrap_or_default())
            })
            .collect()
    }
}

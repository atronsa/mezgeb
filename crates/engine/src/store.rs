//! `PgStore`: the [`Ledger`] operations on PostgreSQL.

use std::collections::BTreeMap;

use mezgeb_core::{
    Account, AccountId, AccountStatus, Authorization, Chart, ClientId, EntryDraft, EntryId, Hold,
    HoldId, Holds, IdempotencyKey, Ledger, LedgerError, PositiveAmount, PostedEntry, PostedLine,
    Timestamp,
};
use sqlx::{PgConnection, PgPool, Row};

use crate::concurrency::PostOptions;
use crate::holds::{self, CaptureHold, HoldRequest, PlaceHold};
use crate::post::{PostRequest, post_entry_with, reverse_entry_with};
use crate::rows::{
    ACCOUNT_COLUMNS, account_from_row, constraint_violation, db, entry_from_row, entry_id,
    line_from_row, storage,
};

/// A ledger stored in PostgreSQL, posting on behalf of one client.
///
/// Connect it as `mezgeb_app` (docs/db-schema.md#roles).
#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
    client: ClientId,
    options: PostOptions,
}

impl PgStore {
    pub fn new(pool: PgPool, client: ClientId) -> PgStore {
        PgStore::with_options(pool, client, PostOptions::default())
    }

    /// A store with explicit concurrency options (step 1.10).
    pub fn with_options(pool: PgPool, client: ClientId, options: PostOptions) -> PgStore {
        PgStore {
            pool,
            client,
            options,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Runs the expiry worker once on this store's pool, on behalf of its
    /// client: expires up to `limit` holds due at `now`
    /// ([`holds::expire_due_holds`]).
    pub async fn expire_due_holds(
        &self,
        now: Timestamp,
        limit: usize,
    ) -> Result<Vec<Hold>, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        holds::expire_due_holds(&mut conn, self.client, now, limit, &self.options).await
    }

    /// The idempotency key used for an entry posted through the `Ledger`
    /// trait, which has no key of its own. Step 1.11 adds client keys.
    fn key_for(id: EntryId) -> Result<IdempotencyKey, LedgerError> {
        IdempotencyKey::new(format!("entry-{id}")).map_err(storage)
    }

    fn hold_request(&self, entry: EntryId) -> Result<HoldRequest, LedgerError> {
        Ok(HoldRequest {
            entry_id: entry,
            client_id: self.client,
            idempotency_key: PgStore::key_for(entry)?,
        })
    }

    /// One attempt at a status change: lock the account, apply the
    /// lifecycle rules from `mezgeb-core`, write the new status.
    async fn set_status_once(
        &self,
        conn: &mut PgConnection,
        id: AccountId,
        to: AccountStatus,
    ) -> Result<(), LedgerError> {
        let mut tx = self.options.begin(conn).await?;
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1{}",
            self.options.lock_clause()
        )))
        .bind(id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or(LedgerError::AccountNotFound(id))?;
        let mut account = account_from_row(&row)?;
        account.set_status(to).map_err(LedgerError::Status)?;
        sqlx::query("UPDATE accounts SET status = $2, version = version + 1 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(to.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)
    }
}

impl Ledger for PgStore {
    async fn open_account(
        &mut self,
        id: AccountId,
        chart: &Chart,
        reference: &str,
    ) -> Result<Account, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1)")
                .bind(id.as_uuid())
                .fetch_one(&mut *conn)
                .await
                .map_err(db)?;
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
            _ => db(e),
        })?;
        Ok(account)
    }

    async fn set_status(&mut self, id: AccountId, to: AccountStatus) -> Result<(), LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let mut attempt = 1;
        loop {
            match self.set_status_once(&mut conn, id, to).await {
                Err(error) if self.options.retry_after(&error, attempt).await => attempt += 1,
                other => return other,
            }
        }
    }

    async fn post(&mut self, id: EntryId, draft: EntryDraft) -> Result<PostedEntry, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let request = PostRequest::new(id, self.client, PgStore::key_for(id)?, draft);
        post_entry_with(&mut conn, request, &self.options).await
    }

    async fn post_adjustment(
        &mut self,
        id: EntryId,
        draft: EntryDraft,
        authorization: Authorization,
    ) -> Result<PostedEntry, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let request = PostRequest::new(id, self.client, PgStore::key_for(id)?, draft)
            .authorized(authorization);
        post_entry_with(&mut conn, request, &self.options).await
    }

    async fn reverse(
        &mut self,
        original: EntryId,
        id: EntryId,
        authorization: Authorization,
    ) -> Result<PostedEntry, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let request = PostRequest::new(
            id,
            self.client,
            PgStore::key_for(id)?,
            EntryDraft::default(),
        )
        .authorized(authorization);
        reverse_entry_with(&mut conn, original, request, &self.options).await
    }

    async fn get_account(&mut self, id: AccountId) -> Result<Option<Account>, LedgerError> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref().map(account_from_row).transpose()
    }

    async fn all_accounts(&mut self) -> Result<Vec<Account>, LedgerError> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY id"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter().map(account_from_row).collect()
    }

    async fn all_entries(&mut self) -> Result<Vec<PostedEntry>, LedgerError> {
        let line_rows = sqlx::query(
            "SELECT entry_id, account_id, side, amount, currency, debits_after, credits_after
               FROM journal_lines ORDER BY entry_id, line_no",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let mut lines: BTreeMap<EntryId, Vec<PostedLine>> = BTreeMap::new();
        for row in &line_rows {
            let entry = entry_id(row.try_get("entry_id").map_err(db)?)?;
            lines.entry(entry).or_default().push(line_from_row(row)?);
        }
        let entry_rows = sqlx::query(
            "SELECT id, sequence, reverses_entry_id, metadata::text AS metadata,
                    (extract(epoch FROM event_time) * 1000000)::bigint AS event_time_micros,
                    kind, reason_code, requested_by, approved_by
               FROM journal_entries ORDER BY sequence",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        entry_rows
            .iter()
            .map(|row| {
                let id = entry_id(row.try_get("id").map_err(db)?)?;
                entry_from_row(row, lines.remove(&id).unwrap_or_default())
            })
            .collect()
    }
}

impl Holds for PgStore {
    async fn place_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        account: AccountId,
        held: AccountId,
        amount: PositiveAmount,
        expires_at: Timestamp,
    ) -> Result<Hold, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let place = PlaceHold {
            hold: id,
            account,
            held,
            amount,
            expires_at,
        };
        holds::place_hold(&mut conn, &self.hold_request(entry)?, place, &self.options).await
    }

    async fn capture_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        destination: AccountId,
        amount: PositiveAmount,
        now: Timestamp,
    ) -> Result<Hold, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let capture = CaptureHold {
            hold: id,
            destination,
            amount,
            now,
        };
        holds::capture_hold(
            &mut conn,
            &self.hold_request(entry)?,
            capture,
            &self.options,
        )
        .await
    }

    async fn release_hold(&mut self, id: HoldId, entry: EntryId) -> Result<Hold, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        holds::release_hold(&mut conn, &self.hold_request(entry)?, id, &self.options).await
    }

    async fn expire_hold(
        &mut self,
        id: HoldId,
        entry: EntryId,
        now: Timestamp,
    ) -> Result<Hold, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        holds::expire_hold(
            &mut conn,
            &self.hold_request(entry)?,
            id,
            now,
            &self.options,
        )
        .await
    }

    async fn all_holds(&mut self) -> Result<Vec<Hold>, LedgerError> {
        let mut conn = self.pool.acquire().await.map_err(db)?;
        holds::all_holds(&mut conn).await
    }
}

//! Internal reconciliation: a client's cached balances against the ledger
//! (ADR-0002, architecture §5.9, ADR-0020 decision 2).
//!
//! The client sends each balance with the posting sequence it was computed
//! at, so an entry posted while the client was reading is not a break: the
//! ledger's balance is taken at the same point. The ledger wins: a
//! disagreement is a `stale_cache` break, and the client refreshes from the
//! balance returned. A later call that agrees clears it.

use std::collections::HashSet;
use std::time::Instant;

use mezgeb_core::{AccountId, ClientId, Timestamp};
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;

use super::store::{NewBreak, RunRow, get_run, insert_breaks, insert_run, lock_scope};
use super::{BreakKind, ReconError, ReconRun, RunKind};
use crate::rows::storage;

/// At most this many balances per call.
pub const MAX_CACHED_BALANCES: usize = 1000;

/// One balance as the client's cache holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedBalance {
    pub account: AccountId,
    /// On the account's normal side, in minor units.
    pub balance_minor: i64,
    /// The posting sequence the cache reflects: the last entry it applied.
    /// `None` compares with the balance now.
    pub as_of_sequence: Option<i64>,
}

/// One comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheResult {
    pub account: AccountId,
    pub cached_minor: i64,
    /// The ledger's balance at `as_of_sequence`.
    pub ledger_minor: i64,
    pub as_of_sequence: i64,
    pub agrees: bool,
}

/// What a cache reconciliation found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheReport {
    pub run: ReconRun,
    /// In the order the balances were given.
    pub results: Vec<CacheResult>,
}

/// Compares `client`'s cached balances with the ledger. The caller checks
/// that the client may read these accounts.
pub async fn reconcile_cache(
    conn: &mut PgConnection,
    client: ClientId,
    balances: &[CachedBalance],
    now: Timestamp,
) -> Result<CacheReport, ReconError> {
    let started = Instant::now();
    if balances.is_empty() || balances.len() > MAX_CACHED_BALANCES {
        return Err(ReconError::Invalid(format!(
            "send 1 to {MAX_CACHED_BALANCES} balances"
        )));
    }
    let mut seen = HashSet::new();
    if let Some(dup) = balances.iter().find(|b| !seen.insert(b.account)) {
        return Err(ReconError::Invalid(format!(
            "account {} is given twice",
            dup.account
        )));
    }
    if balances
        .iter()
        .any(|b| b.as_of_sequence.is_some_and(|s| s < 0))
    {
        return Err(ReconError::Invalid("a sequence is never negative".into()));
    }

    let mut tx = conn.begin().await?;
    // One snapshot for the head of the ledger and every balance.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    lock_scope(&mut tx, &format!("cache:{client}")).await?;
    let head: i64 = sqlx::query_scalar("SELECT coalesce(max(sequence), 0) FROM journal_entries")
        .fetch_one(&mut *tx)
        .await?;
    if let Some(ahead) = balances
        .iter()
        .find(|b| b.as_of_sequence.is_some_and(|s| s > head))
    {
        return Err(ReconError::Invalid(format!(
            "account {}: sequence {} is ahead of the ledger ({head})",
            ahead.account,
            ahead.as_of_sequence.unwrap_or_default()
        )));
    }

    let accounts: Vec<Uuid> = balances.iter().map(|b| b.account.as_uuid()).collect();
    let as_of: Vec<i64> = balances
        .iter()
        .map(|b| b.as_of_sequence.unwrap_or(head))
        .collect();
    let rows = sqlx::query(
        "SELECT u.n, a.id IS NOT NULL AS found,
                CASE WHEN a.account_type IN ('asset', 'expense')
                     THEN (a.debits_total - coalesce(later.debits, 0))
                          - (a.credits_total - coalesce(later.credits, 0))
                     ELSE (a.credits_total - coalesce(later.credits, 0))
                          - (a.debits_total - coalesce(later.debits, 0))
                END::bigint AS balance
           FROM unnest($1::uuid[], $2::bigint[]) WITH ORDINALITY AS u (account, as_of, n)
           LEFT JOIN accounts a ON a.id = u.account
           LEFT JOIN LATERAL (
                SELECT sum(l.amount) FILTER (WHERE l.side = 'debit') AS debits,
                       sum(l.amount) FILTER (WHERE l.side = 'credit') AS credits
                  FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
                 WHERE l.account_id = u.account AND e.sequence > u.as_of
           ) later ON true
          ORDER BY u.n",
    )
    .bind(&accounts)
    .bind(&as_of)
    .fetch_all(&mut *tx)
    .await?;

    let mut results = Vec::with_capacity(balances.len());
    for (cached, row) in balances.iter().zip(&rows) {
        let found: bool = row.try_get("found")?;
        if !found {
            return Err(ReconError::NotFound(format!("account {}", cached.account)));
        }
        let ledger: i64 = row.try_get("balance")?;
        results.push(CacheResult {
            account: cached.account,
            cached_minor: cached.balance_minor,
            ledger_minor: ledger,
            as_of_sequence: cached.as_of_sequence.unwrap_or(head),
            agrees: ledger == cached.balance_minor,
        });
    }

    let run_id = Uuid::now_v7();
    let breaks: Vec<NewBreak> = results
        .iter()
        .filter(|r| !r.agrees)
        .map(|r| NewBreak {
            kind: Some(BreakKind::StaleCache),
            subject: format!("cache:{client}:{}", r.account),
            account: Some(r.account.as_uuid()),
            client: Some(client.as_uuid()),
            other_minor: Some(r.cached_minor),
            ledger_minor: Some(r.ledger_minor),
            difference_minor: Some(r.cached_minor.saturating_sub(r.ledger_minor)),
            detail: format!(
                "the client's cache disagrees with the ledger at sequence {}",
                r.as_of_sequence
            ),
            ..NewBreak::default()
        })
        .collect();
    let breaks_new = insert_breaks(&mut tx, run_id, &breaks, now).await?;
    let agreeing: Vec<Uuid> = results
        .iter()
        .filter(|r| r.agrees)
        .map(|r| r.account.as_uuid())
        .collect();
    let cleared = sqlx::query(
        "UPDATE recon_breaks
            SET status = 'cleared', cleared_by_run_id = $3,
                closed_at = timestamptz 'epoch' + $4::bigint * interval '1 microsecond'
          WHERE status = 'open' AND kind = 'stale_cache' AND client_id = $1
            AND account_id = ANY($2)",
    )
    .bind(client.as_uuid())
    .bind(&agreeing)
    .bind(run_id)
    .bind(now.as_micros())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recon_breaks
          WHERE status = 'open' AND kind = 'stale_cache' AND client_id = $1",
    )
    .bind(client.as_uuid())
    .fetch_one(&mut *tx)
    .await?;

    insert_run(
        &mut tx,
        &RunRow {
            id: run_id,
            kind: RunKind::Internal,
            account: None,
            client: Some(client.as_uuid()),
            source: String::new(),
            as_of: now,
            items: results.len(),
            matched: agreeing.len(),
            pending: 0,
            breaks_new,
            breaks_cleared: usize::try_from(cleared).map_err(storage)?,
            breaks_open: usize::try_from(open).map_err(storage)?,
            started,
        },
    )
    .await?;
    let run = get_run(&mut tx, run_id).await?;
    tx.commit().await?;
    Ok(CacheReport { run, results })
}

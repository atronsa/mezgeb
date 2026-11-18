//! Offline reconciliation: what devices report and what the escrows hold,
//! against the receipts and entries the ledger has (ADR-0020, ADR-0019).
//!
//! - `missing_receipts`: a device reported, when it asked for its next
//!   voucher, a last counter above receipts never received for the
//!   previous one, and that voucher has ended, so they can no longer
//!   arrive. Someone holds signed receipts the ledger never paid.
//! - `counter_behind`: a device reported a last counter below a receipt it
//!   signed under its previous voucher: a reset, restored or cloned device.
//! - `receipt_entry_mismatch`: a settled receipt whose entry does not debit
//!   its amount or does not pay its payee.
//! - `escrow_mismatch`: an escrow that does not hold its voucher's unspent
//!   value (the check of step 2.11).
//!
//! The last two mean the ledger disagrees with its own offline records: a
//! defect. A finding that is gone in a later run clears its break.

use std::time::Instant;

use mezgeb_core::Timestamp;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;

use super::store::{NewBreak, RunRow, get_run, insert_breaks, insert_run, lock_scope};
use super::{BreakKind, ReconError, ReconRun, RunKind};
use crate::offline::escrow_findings;
use crate::rows::storage;

/// Runs the offline reconciliation over every device and voucher.
pub async fn reconcile_offline(
    conn: &mut PgConnection,
    now: Timestamp,
) -> Result<ReconRun, ReconError> {
    let started = Instant::now();
    let mut tx = conn.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    lock_scope(&mut tx, "offline").await?;
    let mut breaks = Vec::new();

    // Counters a device reported, never received, and no longer expected.
    for row in sqlx::query(
        "WITH v AS (
             SELECT id, device_id, counter_start, status,
                    lead(counter_start) OVER w AS next_start,
                    lead(reported_last_counter) OVER w AS next_reported
               FROM vouchers
             WINDOW w AS (PARTITION BY device_id ORDER BY counter_start)
         ), r AS (
             SELECT id, device_id, counter_start,
                    least(next_reported, next_start - 1) AS last_reported
               FROM v
              WHERE status = 'ended' AND next_reported IS NOT NULL
         )
         SELECT r.id, r.device_id, r.counter_start, r.last_reported,
                (SELECT count(*) FROM offline_receipts o
                  WHERE o.device_id = r.device_id
                    AND o.counter BETWEEN r.counter_start AND r.last_reported) AS received
           FROM r
          WHERE r.last_reported >= r.counter_start",
    )
    .fetch_all(&mut *tx)
    .await?
    {
        let voucher: Uuid = row.try_get("id")?;
        let start: i64 = row.try_get("counter_start")?;
        let last: i64 = row.try_get("last_reported")?;
        let received: i64 = row.try_get("received")?;
        let expected = last - start + 1;
        if received < expected {
            breaks.push(NewBreak {
                kind: Some(BreakKind::MissingReceipts),
                subject: format!("voucher:{voucher}:missing"),
                device: Some(row.try_get("device_id")?),
                voucher: Some(voucher),
                detail: format!(
                    "the device reported counters {start} to {last}; {} of them were never \
                     received and the voucher has ended",
                    expected - received
                ),
                ..NewBreak::default()
            });
        }
    }

    // A device that reported less than it had signed.
    for row in sqlx::query(
        "SELECT n.id, n.device_id, n.reported_last_counter,
                (SELECT max(o.counter) FROM offline_receipts o
                  WHERE o.device_id = n.device_id
                    AND o.counter > n.reported_last_counter
                    AND o.counter < n.counter_start) AS above
           FROM vouchers n
          WHERE n.reported_last_counter IS NOT NULL",
    )
    .fetch_all(&mut *tx)
    .await?
    {
        let above: Option<i64> = row.try_get("above")?;
        let Some(above) = above else { continue };
        let voucher: Uuid = row.try_get("id")?;
        let reported: i64 = row.try_get("reported_last_counter")?;
        breaks.push(NewBreak {
            kind: Some(BreakKind::CounterBehind),
            subject: format!("voucher:{voucher}:behind"),
            device: Some(row.try_get("device_id")?),
            voucher: Some(voucher),
            detail: format!(
                "the device reported last counter {reported} when asking for this voucher, \
                 but signed counter {above}"
            ),
            ..NewBreak::default()
        });
    }

    // Settled receipts whose entry does not pay them.
    for row in sqlx::query(
        "SELECT o.device_id, o.counter, o.voucher_id, o.entry_id, o.amount_minor,
                (SELECT coalesce(sum(l.amount), 0) FROM journal_lines l
                  WHERE l.entry_id = o.entry_id AND l.side = 'debit')::bigint AS debits,
                EXISTS (SELECT 1 FROM journal_lines l
                         WHERE l.entry_id = o.entry_id AND l.side = 'credit'
                           AND l.account_id = o.payee_account_id) AS pays_payee
           FROM offline_receipts o
          WHERE o.outcome IN ('settled', 'settled_late')",
    )
    .fetch_all(&mut *tx)
    .await?
    {
        let amount: i64 = row.try_get("amount_minor")?;
        let debits: i64 = row.try_get("debits")?;
        let pays: bool = row.try_get("pays_payee")?;
        if debits == amount && pays {
            continue;
        }
        let device: Uuid = row.try_get("device_id")?;
        let counter: i64 = row.try_get("counter")?;
        breaks.push(NewBreak {
            kind: Some(BreakKind::ReceiptEntryMismatch),
            subject: format!("receipt:{device}:{counter}"),
            device: Some(device),
            voucher: Some(row.try_get("voucher_id")?),
            entry: None,
            other_minor: Some(amount),
            ledger_minor: Some(debits),
            difference_minor: Some(amount - debits),
            detail: if pays {
                "the settling entry debits another amount than the receipt".into()
            } else {
                "the settling entry does not pay the receipt's payee".into()
            },
            ..NewBreak::default()
        });
    }

    // Escrows against their vouchers.
    for finding in escrow_findings(&mut tx).await? {
        breaks.push(NewBreak {
            kind: Some(BreakKind::EscrowMismatch),
            subject: format!("escrow:{}", finding.escrow_account),
            account: Some(finding.escrow_account.as_uuid()),
            voucher: finding.voucher.map(|v| v.as_uuid()),
            other_minor: Some(finding.expected_minor),
            ledger_minor: Some(finding.actual_minor),
            difference_minor: Some(finding.expected_minor - finding.actual_minor),
            detail: "the escrow does not hold its voucher's unspent value".into(),
            ..NewBreak::default()
        });
    }

    let checked: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM vouchers)
              + (SELECT count(*) FROM offline_receipts
                  WHERE outcome IN ('settled', 'settled_late'))",
    )
    .fetch_one(&mut *tx)
    .await?;

    let run_id = Uuid::now_v7();
    let breaks_new = insert_breaks(&mut tx, run_id, &breaks, now).await?;
    let subjects: Vec<&str> = breaks.iter().map(|b| b.subject.as_str()).collect();
    let cleared = sqlx::query(
        "UPDATE recon_breaks
            SET status = 'cleared', cleared_by_run_id = $2,
                closed_at = timestamptz 'epoch' + $3::bigint * interval '1 microsecond'
          WHERE status = 'open'
            AND kind IN ('missing_receipts', 'counter_behind', 'receipt_entry_mismatch',
                         'escrow_mismatch')
            AND NOT (subject = ANY($1))",
    )
    .bind(&subjects)
    .bind(run_id)
    .bind(now.as_micros())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recon_breaks
          WHERE status = 'open'
            AND kind IN ('missing_receipts', 'counter_behind', 'receipt_entry_mismatch',
                         'escrow_mismatch')",
    )
    .fetch_one(&mut *tx)
    .await?;

    insert_run(
        &mut tx,
        &RunRow {
            id: run_id,
            kind: RunKind::Offline,
            account: None,
            client: None,
            source: String::new(),
            as_of: now,
            items: usize::try_from(checked).map_err(storage)?,
            matched: 0,
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
    Ok(run)
}

//! The rest of a voucher's life on PostgreSQL (step 2.13, ADR-0019,
//! `docs/offline/protocol.md` §7, §11, §12, §14, migration 0013).
//!
//! - [`offline_tick`] moves time forward for every voucher that is due:
//!   - **the gap timeout:** receipts that have waited 72 hours for a missing
//!     predecessor settle past it, in counter order, marked `gap_skipped`;
//!   - **expiry:** `ACTIVE` becomes `GRACE` at `expires_at`;
//!   - **the return:** after 7 days of grace, the waiting receipts settle
//!     past their gaps, then the escrow left goes back to the wallet (E18),
//!     and the voucher is `RETURNED`;
//!   - **the end:** after the client's late deadline (90 days by default),
//!     `ENDED`, and late receipts are `EXPIRED`.
//! - [`repay_shortfall`]: the payer repays what the loss account paid for
//!   them (E30); until then no device of theirs gets a voucher.
//! - [`resolve_dispute`]: an operator ends a frozen voucher's case:
//!   `paid` pays every valid receipt in counter order, from the escrow left,
//!   then the payer's wallet, then 5030 (E17 shape, E26); a conflicting
//!   copy of a counter is paid from the wallet and 5030 only (E31); `refused`
//!   pays none. Either way the escrow left returns (E27) and the device is
//!   revoked.
//!
//! Each voucher is handled in its own transaction under its device's and
//! its own row locks, the same order as receipt ingestion, so a receipt
//! racing the expiry has exactly one outcome.

use std::fmt::Write as _;

use mezgeb_core::{
    AccountId, ClientId, DeviceId, EntryId, LedgerError, Money, Side, Timestamp, VoucherId,
};
use mezgeb_offline::{GRACE_MICROS, Outcome, Receipt};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::concurrency::PostOptions;
use crate::offline::{IssuedVoucher, OfflineError, VoucherStatus, load_device, sql};
use crate::offline_receipts::{
    Ctx, OfflineAccounts, OfflineBooks, Parts, RepayRequest, ReturnRequest, Settled, ToPay,
    balance_of, db_outcome, has_lines, load_stored, lock_voucher, names, pay_receipt, post_offline,
    record_paid, settle_waiting,
};
use crate::rows::{account_id, db, entry_id, storage};

/// How long a gap is waited for (protocol §11).
pub const GAP_TIMEOUT_MICROS: i64 = 72 * 3_600 * 1_000_000;

/// What one tick did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Receipts settled past a gap, by voucher.
    pub settled_past_gaps: Vec<(VoucherId, Vec<Settled>)>,
    /// Vouchers that went from `ACTIVE` to `GRACE`.
    pub graced: Vec<VoucherId>,
    /// Vouchers whose escrow went back, with the entry (none if it was
    /// already zero).
    pub returned: Vec<(VoucherId, Option<EntryId>)>,
    /// Vouchers past their late deadline.
    pub ended: Vec<VoucherId>,
}

fn micros(t: Timestamp) -> i64 {
    t.as_micros()
}

/// The vouchers with something due at `now`.
async fn due(
    conn: &mut PgConnection,
    now: Timestamp,
) -> Result<Vec<(DeviceId, VoucherId)>, LedgerError> {
    let rows = sqlx::query(
        "WITH t AS (SELECT timestamptz 'epoch' + $1::bigint * interval '1 microsecond' AS now)
         SELECT v.device_id, v.id
           FROM vouchers v CROSS JOIN t
           LEFT JOIN offline_settings s ON s.client_id = v.client_id
          WHERE (v.status = 'active' AND v.expires_at <= t.now)
             OR (v.status = 'grace'
                 AND v.expires_at + $2::bigint * interval '1 microsecond' <= t.now)
             OR (v.status IN ('returned', 'closed')
                 AND v.returned_at + COALESCE(s.late_deadline_days, 90) * interval '1 day' <= t.now)
             OR (v.status IN ('active', 'grace') AND EXISTS (
                     SELECT 1 FROM offline_receipts r
                      WHERE r.voucher_id = v.id AND r.outcome = 'waiting_for_gap'
                        AND r.received_at + $3::bigint * interval '1 microsecond' <= t.now))
          ORDER BY v.id",
    )
    .bind(micros(now))
    .bind(GRACE_MICROS)
    .bind(GAP_TIMEOUT_MICROS)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter()
        .map(|r| {
            Ok((
                DeviceId::from_uuid(r.try_get("device_id").map_err(db)?).map_err(storage)?,
                VoucherId::from_uuid(r.try_get("id").map_err(db)?).map_err(storage)?,
            ))
        })
        .collect()
}

/// Whether a receipt of the voucher has waited past the gap timeout.
async fn gap_timed_out(
    conn: &mut PgConnection,
    voucher: VoucherId,
    now: Timestamp,
) -> Result<bool, LedgerError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM offline_receipts
                         WHERE voucher_id = $1 AND outcome = 'waiting_for_gap'
                           AND received_at + $3::bigint * interval '1 microsecond'
                               <= timestamptz 'epoch' + $2::bigint * interval '1 microsecond')",
    )
    .bind(voucher.as_uuid())
    .bind(micros(now))
    .bind(GAP_TIMEOUT_MICROS)
    .fetch_one(&mut *conn)
    .await
    .map_err(db)
}

/// Returns the escrow left to the wallet (E18, E25, E27) and marks the
/// voucher `RETURNED`. The entry is `None` if the escrow was already zero.
pub(crate) async fn return_escrow<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    ctx: &Ctx<'_, B>,
    voucher: &IssuedVoucher,
    reason: &'static str,
) -> Result<Option<EntryId>, OfflineError> {
    let escrow = voucher.voucher.escrow_account_id;
    let wallet = voucher.owner_account;
    let left = balance_of(conn, escrow).await?;
    let entry = if left > 0 {
        let request = ReturnRequest {
            escrow,
            wallet,
            amount: Money::from_minor(left, voucher.voucher.amount.currency()),
            voucher: voucher.voucher.voucher_id,
            reason,
        };
        let view = crate::offline_receipts::view(conn, &[escrow, wallet]).await?;
        let draft = ctx
            .books
            .return_escrow(&view, &request)
            .map_err(OfflineError::Booking)?;
        if !has_lines(
            &draft,
            vec![
                (escrow, Side::Debit, request.amount),
                (wallet, Side::Credit, request.amount),
            ],
        ) || !names(&draft, &[("voucher", request.voucher.to_string())])
        {
            return Err(OfflineError::Booking(
                "the booking does not return the escrow to its wallet".into(),
            ));
        }
        Some(
            post_offline(
                conn,
                voucher.voucher.client_id,
                &format!("offline-return:{}", voucher.voucher.voucher_id),
                draft,
                ctx.options,
            )
            .await?,
        )
    } else {
        None
    };
    sqlx::query(
        "UPDATE vouchers
            SET status = 'returned', status_changed_at = clock_timestamp(), return_entry_id = $2,
                returned_at = timestamptz 'epoch' + $3::bigint * interval '1 microsecond'
          WHERE id = $1",
    )
    .bind(voucher.voucher.voucher_id.as_uuid())
    .bind(entry.map(EntryId::as_uuid))
    .bind(micros(ctx.now))
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(entry)
}

async fn reload(
    conn: &mut PgConnection,
    voucher: &IssuedVoucher,
) -> Result<IssuedVoucher, OfflineError> {
    lock_voucher(conn, voucher.voucher.device_id, voucher.voucher.voucher_id)
        .await?
        .ok_or(OfflineError::VoucherNotFound)
}

/// Everything due for one voucher, under its device's and its own locks.
async fn tick_voucher<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    ctx: &Ctx<'_, B>,
    device: DeviceId,
    voucher_id: VoucherId,
    report: &mut TickReport,
) -> Result<(), OfflineError> {
    load_device(conn, None, device, true).await?;
    let Some(mut voucher) = lock_voucher(conn, device, voucher_id).await? else {
        return Ok(());
    };
    let now = ctx.now;
    let open = |v: &IssuedVoucher| matches!(v.status, VoucherStatus::Active | VoucherStatus::Grace);

    // The gap timeout.
    if open(&voucher) && gap_timed_out(conn, voucher_id, now).await? {
        let settled = settle_waiting(conn, ctx, &voucher, true).await?;
        if !settled.is_empty() {
            report.settled_past_gaps.push((voucher_id, settled));
        }
        voucher = reload(conn, &voucher).await?;
    }
    // Expiry.
    if voucher.status == VoucherStatus::Active && voucher.voucher.expires_at <= now {
        sqlx::query(
            "UPDATE vouchers SET status = 'grace', status_changed_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(voucher_id.as_uuid())
        .execute(&mut *conn)
        .await
        .map_err(sql)?;
        report.graced.push(voucher_id);
        voucher = reload(conn, &voucher).await?;
    }
    // The return, once grace ends: what waits settles past its gaps first.
    if voucher.status == VoucherStatus::Grace
        && voucher.voucher.expires_at.as_micros() + GRACE_MICROS <= now.as_micros()
    {
        let settled = settle_waiting(conn, ctx, &voucher, true).await?;
        if !settled.is_empty() {
            report.settled_past_gaps.push((voucher_id, settled));
        }
        voucher = reload(conn, &voucher).await?;
        if voucher.status == VoucherStatus::Grace {
            let entry = return_escrow(conn, ctx, &voucher, "expired").await?;
            report.returned.push((voucher_id, entry));
        }
    }
    // The end, after the late deadline.
    let ended = sqlx::query(
        "UPDATE vouchers v
            SET status = 'ended', status_changed_at = clock_timestamp(),
                ended_at = timestamptz 'epoch' + $2::bigint * interval '1 microsecond'
           FROM (SELECT v2.id, COALESCE(s.late_deadline_days, 90) AS days
                   FROM vouchers v2 LEFT JOIN offline_settings s ON s.client_id = v2.client_id
                  WHERE v2.id = $1) d
          WHERE v.id = d.id AND v.status IN ('returned', 'closed')
            AND v.returned_at + d.days * interval '1 day'
                <= timestamptz 'epoch' + $2::bigint * interval '1 microsecond'",
    )
    .bind(voucher_id.as_uuid())
    .bind(micros(now))
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    if ended.rows_affected() > 0 {
        report.ended.push(voucher_id);
    }
    Ok(())
}

/// Moves every due voucher forward to `now`: gap timeouts, expiry, the
/// return of escrow and the late deadline. Safe to run at any time and as
/// often as wanted: what it did stays done, and nothing is done twice. The
/// service runs it every minute.
pub async fn offline_tick<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    now: Timestamp,
    accounts: OfflineAccounts,
    books: &B,
    options: &PostOptions,
) -> Result<TickReport, OfflineError> {
    let ctx = Ctx {
        books,
        accounts,
        now,
        options,
    };
    let mut report = TickReport::default();
    for (device, voucher) in due(conn, now).await? {
        let mut attempt = 1;
        loop {
            let mut part = TickReport::default();
            let result = async {
                let mut tx = options.begin(conn).await?;
                match tick_voucher(&mut tx, &ctx, device, voucher, &mut part).await {
                    Ok(()) => {
                        options.commit(tx).await?;
                        Ok(())
                    }
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }
            .await;
            match result {
                Ok(()) => {
                    report.settled_past_gaps.extend(part.settled_past_gaps);
                    report.graced.extend(part.graced);
                    report.returned.extend(part.returned);
                    report.ended.extend(part.ended);
                    break;
                }
                Err(OfflineError::Ledger(error)) if options.retry_after(&error, attempt).await => {
                    attempt += 1;
                }
                Err(other) => return Err(other),
            }
        }
    }
    Ok(report)
}

/// What a payer could not pay of a late receipt, paid by the loss account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortfall {
    pub id: Uuid,
    pub client: ClientId,
    /// The payer's wallet.
    pub owner_account: AccountId,
    pub device: DeviceId,
    pub counter: u64,
    pub amount: Money,
    /// The entry that paid it from 5030 (E29).
    pub entry: EntryId,
    /// The entry that repaid it (E30), if it is repaid.
    pub repaid_entry: Option<EntryId>,
}

const SHORTFALL_COLUMNS: &str = "id, client_id, owner_account_id, device_id, counter, currency, amount_minor, entry_id, \
     repaid_entry_id";

fn shortfall_from_row(row: &PgRow) -> Result<Shortfall, LedgerError> {
    let currency: String = row.try_get("currency").map_err(db)?;
    let counter: i64 = row.try_get("counter").map_err(db)?;
    let repaid: Option<Uuid> = row.try_get("repaid_entry_id").map_err(db)?;
    Ok(Shortfall {
        id: row.try_get("id").map_err(db)?,
        client: ClientId::from_uuid(row.try_get("client_id").map_err(db)?).map_err(storage)?,
        owner_account: account_id(row.try_get("owner_account_id").map_err(db)?)?,
        device: DeviceId::from_uuid(row.try_get("device_id").map_err(db)?).map_err(storage)?,
        counter: u64::try_from(counter).map_err(storage)?,
        amount: Money::from_minor(
            row.try_get("amount_minor").map_err(db)?,
            mezgeb_core::Currency::from_code(&currency).map_err(storage)?,
        ),
        entry: entry_id(row.try_get("entry_id").map_err(db)?)?,
        repaid_entry: repaid.map(entry_id).transpose()?,
    })
}

/// The shortfalls of `client`'s customers, oldest first; with `owner`, one
/// wallet's.
pub async fn shortfalls(
    conn: &mut PgConnection,
    client: ClientId,
    owner: Option<AccountId>,
) -> Result<Vec<Shortfall>, OfflineError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {SHORTFALL_COLUMNS} FROM offline_shortfalls
          WHERE client_id = $1 AND ($2::uuid IS NULL OR owner_account_id = $2)
          ORDER BY created_at, id"
    )))
    .bind(client.as_uuid())
    .bind(owner.map(AccountId::as_uuid))
    .fetch_all(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(rows
        .iter()
        .map(shortfall_from_row)
        .collect::<Result<_, _>>()?)
}

/// The payer repays a shortfall from their wallet to the loss account
/// (E30). Repaying a repaid shortfall returns it unchanged. The wallet must
/// hold the amount (INV-8).
pub async fn repay_shortfall<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    client: ClientId,
    id: Uuid,
    accounts: OfflineAccounts,
    books: &B,
    options: &PostOptions,
) -> Result<Shortfall, OfflineError> {
    let mut tx = options.begin(conn).await?;
    let result = async {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {SHORTFALL_COLUMNS} FROM offline_shortfalls
              WHERE id = $1 AND client_id = $2 FOR UPDATE"
        )))
        .bind(id)
        .bind(client.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(sql)?;
        let shortfall = shortfall_from_row(&row.ok_or(OfflineError::ShortfallNotFound)?)?;
        if shortfall.repaid_entry.is_some() {
            return Ok(shortfall);
        }
        let loss = accounts
            .loss_account
            .ok_or_else(|| OfflineError::Booking("there is no loss account to repay".into()))?;
        let request = RepayRequest {
            wallet: shortfall.owner_account,
            loss_account: loss,
            amount: shortfall.amount,
            shortfall: id,
        };
        let view = crate::offline_receipts::view(&mut tx, &[request.wallet, loss]).await?;
        let draft = books
            .repay(&view, &request)
            .map_err(OfflineError::Booking)?;
        if !has_lines(
            &draft,
            vec![
                (request.wallet, Side::Debit, request.amount),
                (loss, Side::Credit, request.amount),
            ],
        ) || !names(&draft, &[("shortfall", id.to_string())])
        {
            return Err(OfflineError::Booking(
                "the booking does not repay the shortfall".into(),
            ));
        }
        let entry = post_offline(
            &mut tx,
            client,
            &format!("offline-repay:{id}"),
            draft,
            options,
        )
        .await?;
        sqlx::query(
            "UPDATE offline_shortfalls SET repaid_entry_id = $2, repaid_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(id)
        .bind(entry.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(sql)?;
        Ok(Shortfall {
            repaid_entry: Some(entry),
            ..shortfall
        })
    }
    .await;
    match result {
        Ok(shortfall) => {
            options.commit(tx).await?;
            Ok(shortfall)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

/// How an operator ends a dispute (protocol §14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeResolution {
    /// Every valid receipt is paid, in counter order: from the escrow left,
    /// then the payer's wallet, then 5030. The payer is liable for what
    /// their device signed.
    Paid,
    /// No receipt is paid: a fault in the ledger or the chip, decided by
    /// the operator. The payees bear them.
    Refused,
}

impl DisputeResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            DisputeResolution::Paid => "paid",
            DisputeResolution::Refused => "refused",
        }
    }

    pub fn parse(text: &str) -> Option<DisputeResolution> {
        match text {
            "paid" => Some(DisputeResolution::Paid),
            "refused" => Some(DisputeResolution::Refused),
            _ => None,
        }
    }
}

/// A frozen device's case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispute {
    pub id: Uuid,
    pub client: ClientId,
    pub device: DeviceId,
    pub voucher: VoucherId,
    /// `conflict` or `over_limit`.
    pub reason: String,
    /// The counter that froze it.
    pub counter: u64,
    pub resolution: Option<DisputeResolution>,
    pub resolved_by: Option<String>,
}

const DISPUTE_COLUMNS: &str =
    "id, client_id, device_id, voucher_id, reason, counter, resolution, resolved_by";

fn dispute_from_row(row: &PgRow) -> Result<Dispute, LedgerError> {
    let counter: i64 = row.try_get("counter").map_err(db)?;
    let resolution: Option<String> = row.try_get("resolution").map_err(db)?;
    Ok(Dispute {
        id: row.try_get("id").map_err(db)?,
        client: ClientId::from_uuid(row.try_get("client_id").map_err(db)?).map_err(storage)?,
        device: DeviceId::from_uuid(row.try_get("device_id").map_err(db)?).map_err(storage)?,
        voucher: VoucherId::from_uuid(row.try_get("voucher_id").map_err(db)?).map_err(storage)?,
        reason: row.try_get("reason").map_err(db)?,
        counter: u64::try_from(counter).map_err(storage)?,
        resolution: resolution
            .map(|r| DisputeResolution::parse(&r).ok_or_else(|| storage(format!("resolution {r}"))))
            .transpose()?,
        resolved_by: row.try_get("resolved_by").map_err(db)?,
    })
}

/// Disputes, newest first: every client's for an operator (`None`), or
/// one client's; only open ones with `open_only`.
pub async fn disputes(
    conn: &mut PgConnection,
    client: Option<ClientId>,
    open_only: bool,
) -> Result<Vec<Dispute>, OfflineError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DISPUTE_COLUMNS} FROM offline_disputes
          WHERE ($1::uuid IS NULL OR client_id = $1) AND (NOT $2 OR status = 'open')
          ORDER BY opened_at DESC, id DESC"
    )))
    .bind(client.map(ClientId::as_uuid))
    .bind(open_only)
    .fetch_all(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(rows
        .iter()
        .map(dispute_from_row)
        .collect::<Result<_, _>>()?)
}

/// One dispute.
pub async fn get_dispute(
    conn: &mut PgConnection,
    client: Option<ClientId>,
    id: Uuid,
) -> Result<Dispute, OfflineError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DISPUTE_COLUMNS} FROM offline_disputes
          WHERE id = $1 AND ($2::uuid IS NULL OR client_id = $2)"
    )))
    .bind(id)
    .bind(client.map(ClientId::as_uuid))
    .fetch_optional(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(dispute_from_row(
        &row.ok_or(OfflineError::DisputeNotFound)?,
    )?)
}

/// The unsettled receipts of a voucher, and the conflicting copies of its
/// counters, in counter order: what a `paid` resolution pays.
struct Unpaid {
    counter: u64,
    payee: AccountId,
    amount: Money,
    event_time: Timestamp,
    /// The conflicting copy's ID and digest, or `None` for the recorded
    /// receipt.
    copy: Option<(i64, [u8; 32])>,
}

async fn unpaid(
    conn: &mut PgConnection,
    voucher: &IssuedVoucher,
) -> Result<Vec<Unpaid>, OfflineError> {
    let device = voucher.voucher.device_id;
    let mut out = Vec::new();
    let counters: Vec<i64> = sqlx::query_scalar(
        "SELECT counter FROM offline_receipts
          WHERE voucher_id = $1
            AND outcome IN ('frozen', 'over_limit', 'conflict', 'waiting_for_gap')
          ORDER BY counter",
    )
    .bind(voucher.voucher.voucher_id.as_uuid())
    .fetch_all(&mut *conn)
    .await
    .map_err(sql)?;
    for counter in counters {
        let counter = u64::try_from(counter).map_err(storage)?;
        let stored = load_stored(conn, device, counter)
            .await?
            .ok_or_else(|| storage("an unpaid receipt disappeared"))?;
        out.push(Unpaid {
            counter,
            payee: stored.payee,
            amount: stored.amount,
            event_time: stored.event_time,
            copy: None,
        });
    }
    let copies = sqlx::query(
        "SELECT c.id, c.body, c.digest FROM offline_receipt_conflicts c
          WHERE c.device_id = $1
            AND NOT EXISTS (SELECT 1 FROM offline_dispute_payments p WHERE p.conflict_id = c.id)
          ORDER BY c.counter, c.id",
    )
    .bind(device.as_uuid())
    .fetch_all(&mut *conn)
    .await
    .map_err(sql)?;
    for row in copies {
        let body: Vec<u8> = row.try_get("body").map_err(db)?;
        let receipt = Receipt::decode(&body).map_err(storage)?;
        if receipt.voucher_id != voucher.voucher.voucher_id {
            continue;
        }
        let digest: Vec<u8> = row.try_get("digest").map_err(db)?;
        out.push(Unpaid {
            counter: receipt.counter,
            payee: receipt.payee_account_id,
            amount: receipt.amount,
            event_time: receipt.event_time,
            copy: Some((
                row.try_get("id").map_err(db)?,
                digest
                    .try_into()
                    .map_err(|_| storage("a digest is not 32 bytes"))?,
            )),
        });
    }
    // Counter order; the recorded receipt before its conflicting copies.
    out.sort_by_key(|u| (u.counter, u.copy.map(|c| c.0)));
    Ok(out)
}

/// Ends a dispute (protocol §14). `paid` pays every valid receipt of the
/// voucher in counter order, from the escrow left, then the payer's
/// wallet, then 5030, recording any shortfall; `refused` pays none. Then
/// the escrow left returns (E27), the voucher is `RETURNED` and the device
/// revoked. Resolving it again the same way returns it unchanged.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn resolve_dispute<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    id: Uuid,
    resolution: DisputeResolution,
    actor: &str,
    now: Timestamp,
    accounts: OfflineAccounts,
    books: &B,
    options: &PostOptions,
) -> Result<Dispute, OfflineError> {
    let ctx = Ctx {
        books,
        accounts,
        now,
        options,
    };
    let found = get_dispute(conn, None, id).await?;
    let mut tx = options.begin(conn).await?;
    let result = async {
        load_device(&mut tx, None, found.device, true).await?;
        let voucher = lock_voucher(&mut tx, found.device, found.voucher)
            .await?
            .ok_or(OfflineError::VoucherNotFound)?;
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {DISPUTE_COLUMNS} FROM offline_disputes WHERE id = $1 FOR UPDATE"
        )))
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(sql)?;
        let dispute = dispute_from_row(&row)?;
        if let Some(done) = dispute.resolution {
            return if done == resolution {
                Ok(dispute)
            } else {
                Err(OfflineError::DisputeResolved(done.as_str()))
            };
        }
        let device = voucher.voucher.device_id;
        let mut paid = 0usize;
        match resolution {
            DisputeResolution::Paid => {
                for item in unpaid(&mut tx, &voucher).await? {
                    let key = match item.copy {
                        None => format!("offline:{device}:{}", item.counter),
                        Some((_, digest)) => format!(
                            "offline:{device}:{}:{}",
                            item.counter,
                            &mezgeb_core::chain::hex(&digest)[..16]
                        ),
                    };
                    let (entry, parts) = pay_receipt(
                        &mut tx,
                        &ctx,
                        &voucher,
                        &ToPay {
                            device,
                            counter: item.counter,
                            payee: item.payee,
                            amount: item.amount,
                            event_time: item.event_time,
                            key,
                            // Escrow never pays one counter twice (INV-21):
                            // a conflicting copy is paid from the wallet and
                            // 5030 only (E31).
                            use_escrow: item.copy.is_none(),
                        },
                    )
                    .await?;
                    match item.copy {
                        None => {
                            let outcome = if parts.escrow == item.amount.amount() {
                                Outcome::Settled
                            } else {
                                Outcome::SettledLate
                            };
                            record_paid(
                                &mut tx,
                                device,
                                item.counter,
                                outcome,
                                entry,
                                parts,
                                false,
                            )
                            .await?;
                        }
                        Some((conflict, _)) => {
                            record_copy(&mut tx, id, conflict, entry, parts).await?;
                        }
                    }
                    paid += 1;
                }
            }
            DisputeResolution::Refused => {
                sqlx::query(
                    "UPDATE offline_receipts SET outcome = $2
                      WHERE voucher_id = $1
                        AND outcome IN ('frozen', 'over_limit', 'conflict', 'waiting_for_gap')",
                )
                .bind(voucher.voucher.voucher_id.as_uuid())
                .bind(db_outcome(Outcome::Refused))
                .execute(&mut *tx)
                .await
                .map_err(sql)?;
            }
        }
        // The escrow left goes back, and the device is retired.
        let voucher = reload(&mut tx, &voucher).await?;
        return_escrow(&mut tx, &ctx, &voucher, "dispute").await?;
        sqlx::query(
            "UPDATE devices SET status = 'revoked', status_reason = $2,
                    status_changed_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(device.as_uuid())
        .bind(format!("dispute {id}: {}", resolution.as_str()))
        .execute(&mut *tx)
        .await
        .map_err(sql)?;
        sqlx::query(
            "UPDATE offline_disputes
                SET status = 'resolved', resolution = $2, resolved_by = $3,
                    resolved_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(id)
        .bind(resolution.as_str())
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(sql)?;
        let mut details = String::new();
        let _ = write!(
            details,
            r#"{{"dispute":"{id}","resolution":"{}","paid":{paid}}}"#,
            resolution.as_str()
        );
        let event = crate::tamper::NewAuditEvent {
            client: Some(dispute.client),
            actor: Some(actor.to_owned()),
            action: "offline.dispute".into(),
            target: Some(format!("device:{device}")),
            request_id: None,
            outcome: "ok".into(),
            details,
        };
        crate::tamper::append_audit(&mut tx, &event, now).await?;
        Ok(Dispute {
            resolution: Some(resolution),
            resolved_by: Some(actor.to_owned()),
            ..dispute
        })
    }
    .await;
    match result {
        Ok(dispute) => {
            options.commit(tx).await?;
            Ok(dispute)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

async fn record_copy(
    conn: &mut PgConnection,
    dispute: Uuid,
    conflict: i64,
    entry: EntryId,
    parts: Parts,
) -> Result<(), OfflineError> {
    sqlx::query(
        "INSERT INTO offline_dispute_payments
             (dispute_id, conflict_id, entry_id, from_escrow_minor, from_wallet_minor,
              from_loss_minor)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(dispute)
    .bind(conflict)
    .bind(entry.as_uuid())
    .bind(parts.escrow)
    .bind(parts.wallet)
    .bind(parts.loss)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(())
}

//! Offline receipt ingestion and settlement on PostgreSQL (step 2.12,
//! ADR-0019, `docs/offline/protocol.md` §9–§11, migration 0012).
//!
//! [`ingest_receipt`] takes one uploaded receipt and gives it one
//! [`Outcome`], in the protocol's order:
//!
//! | # | Check | Outcome if it fails |
//! |---|---|---|
//! | 1 | It decodes | `BAD_FORMAT` |
//! | 2 | The device is the uploader's client's | `UNKNOWN_DEVICE` |
//! | 3 | The signature verifies with the device's key, low-S | `BAD_SIGNATURE` |
//! | 4 | The voucher is the device's, `counter ≥ counter_start` | `UNKNOWN_VOUCHER` |
//! | 5 | Already recorded with the same digest | `DUPLICATE` |
//! | 6 | Already recorded with another digest | `CONFLICT`, freeze |
//! | 7 | The device or voucher is frozen | `FROZEN` |
//! | 8 | The previous counter has not settled | `WAITING_FOR_GAP` |
//! | 9 | `prev_digest` is the previous receipt's (or the voucher's) | `CONFLICT`, freeze |
//! | 10 | The amount fits in the escrow left | `SETTLED` (E17) |
//! | 11 | It does not | `OVER_LIMIT`, freeze |
//!
//! Nothing is written before the signature verifies. A receipt that passes
//! check 4 is stored the first time it arrives, whatever its outcome; that
//! record makes every later upload a `DUPLICATE` or a `CONFLICT`, whoever
//! sends it (INV-20, INV-21). Settling a receipt also settles the receipts
//! that were waiting for it, in counter order, so the final state does not
//! depend on the upload order.
//!
//! Everything for one device happens under its row lock, so concurrent
//! uploads of its receipts queue, and the same receipt uploaded a hundred
//! times at once settles once. The settlement entry's idempotency key is
//! `offline:<device_id>:<counter>`.
//!
//! The engine knows nothing about templates: the caller books E17 through
//! the `book` function, and the engine checks the draft pays exactly the
//! amount from the escrow, less the client's fee.
//!
//! Gap timeouts, voucher expiry and late receipts are step 2.13.

use std::collections::BTreeMap;

use mezgeb_core::chain::hex;
use mezgeb_core::{
    Account, AccountId, ClientId, Currency, DeviceId, EntryDraft, EntryId, IdempotencyKey,
    LedgerError, Money, Side, Timestamp, VoucherId,
};
use mezgeb_offline::{Outcome, Receipt};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::concurrency::PostOptions;
use crate::offline::{
    DeviceStatus, IssuedVoucher, OfflineError, VOUCHER_COLUMNS, VoucherStatus, load_device, sql,
    voucher_from_row,
};
use crate::post::{Claim, PostRequest, claim_key, post_in_transaction};
use crate::rows::{db, entry_id, storage};

/// One uploaded receipt: the bytes the device signed, and its signature.
#[derive(Debug, Clone, Copy)]
pub struct Upload<'a> {
    /// The uploader; the device must be this client's.
    pub client: ClientId,
    pub body: &'a [u8],
    pub signature: &'a [u8],
    /// Ledger time.
    pub now: Timestamp,
}

/// Where the merchant fee goes, if the client charges one.
#[derive(Debug, Clone, Copy, Default)]
pub struct FeePolicy {
    /// 4010 Payment fee revenue. Needed only when a fee is due.
    pub fee_account: Option<AccountId>,
}

/// What `book` is asked to draft: E17 for one receipt.
#[derive(Debug, Clone)]
pub struct SettleRequest {
    pub escrow: AccountId,
    pub payee: AccountId,
    pub fee_account: Option<AccountId>,
    pub amount: Money,
    /// The merchant fee, possibly zero.
    pub fee: Money,
    pub voucher: VoucherId,
    pub device: DeviceId,
    pub counter: u64,
    /// The device's time, if it lies between the voucher's issue and now.
    pub event_time: Option<Timestamp>,
    /// The device's time as it claimed it, when that is outside the bounds.
    pub claimed_time: Option<Timestamp>,
}

/// A receipt settled by an upload: the uploaded one, or one that was
/// waiting for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settled {
    pub counter: u64,
    pub entry: EntryId,
}

/// What happened to one uploaded receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestResult {
    pub outcome: Outcome,
    /// Known once the body decodes.
    pub device: Option<DeviceId>,
    pub counter: Option<u64>,
    pub digest: Option<[u8; 32]>,
    /// The entry that paid it, if it is settled (now or before).
    pub entry: Option<EntryId>,
    /// Why, for a refusal.
    pub detail: Option<String>,
    /// Every receipt this upload settled, in counter order: this one, and
    /// those that were waiting for it.
    pub settled: Vec<Settled>,
}

impl IngestResult {
    fn refused(outcome: Outcome, receipt: Option<&Receipt>, detail: impl Into<String>) -> Self {
        IngestResult {
            outcome,
            device: receipt.map(|r| r.device_id),
            counter: receipt.map(|r| r.counter),
            digest: receipt.map(Receipt::digest),
            entry: None,
            detail: Some(detail.into()),
            settled: Vec::new(),
        }
    }
}

/// A client's merchant fee on offline payments, in basis points (0 without
/// a setting).
pub async fn offline_fee_bps(
    conn: &mut PgConnection,
    client: ClientId,
) -> Result<u32, LedgerError> {
    let bps: Option<i32> =
        sqlx::query_scalar("SELECT fee_bps FROM offline_settings WHERE client_id = $1")
            .bind(client.as_uuid())
            .fetch_optional(&mut *conn)
            .await
            .map_err(db)?;
    Ok(bps.and_then(|b| u32::try_from(b).ok()).unwrap_or(0))
}

/// Sets a client's merchant fee on offline payments, in basis points, at
/// most 1,000 (10%). For `mezgeb-cli offline fee`, as `mezgeb_admin`.
pub async fn set_offline_fee(
    conn: &mut PgConnection,
    client: ClientId,
    bps: u32,
) -> Result<(), OfflineError> {
    let bps = i32::try_from(bps)
        .ok()
        .filter(|b| *b <= 1_000)
        .ok_or_else(|| OfflineError::Invalid("the fee is at most 1000 basis points".into()))?;
    sqlx::query(
        "INSERT INTO offline_settings (client_id, fee_bps) VALUES ($1, $2)
         ON CONFLICT (client_id) DO UPDATE
            SET fee_bps = EXCLUDED.fee_bps, updated_at = clock_timestamp()",
    )
    .bind(client.as_uuid())
    .bind(bps)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(())
}

/// The fee on `amount` at `bps`, rounded down: a fraction of a santim is
/// the payee's.
fn fee_on(amount: Money, bps: u32) -> Money {
    let minor = i128::from(amount.amount()) * i128::from(bps) / 10_000;
    Money::from_minor(i64::try_from(minor).unwrap_or(0), amount.currency())
}

/// A stored receipt, as the checks need it.
#[derive(Debug, Clone)]
struct Stored {
    digest: [u8; 32],
    prev_digest: [u8; 32],
    payee: AccountId,
    amount: Money,
    event_time: Timestamp,
    outcome: Outcome,
    entry: Option<EntryId>,
}

fn stored_outcome(text: &str) -> Result<Outcome, LedgerError> {
    Outcome::parse(&text.to_uppercase()).ok_or_else(|| storage(format!("outcome {text:?}")))
}

fn db_outcome(outcome: Outcome) -> String {
    outcome.as_str().to_lowercase()
}

async fn load_stored(
    conn: &mut PgConnection,
    device: DeviceId,
    counter: u64,
) -> Result<Option<Stored>, LedgerError> {
    let row = sqlx::query(
        "SELECT digest, prev_digest, payee_account_id, currency, amount_minor, outcome, entry_id,
                (extract(epoch FROM event_time) * 1000000)::bigint AS event_micros
           FROM offline_receipts WHERE device_id = $1 AND counter = $2",
    )
    .bind(device.as_uuid())
    .bind(i64::try_from(counter).map_err(storage)?)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    let Some(row) = row else { return Ok(None) };
    let digest32 = |name: &str| -> Result<[u8; 32], LedgerError> {
        let bytes: Vec<u8> = row.try_get(name).map_err(db)?;
        bytes
            .try_into()
            .map_err(|_| storage(format!("{name} is not 32 bytes")))
    };
    let currency: String = row.try_get("currency").map_err(db)?;
    let entry: Option<Uuid> = row.try_get("entry_id").map_err(db)?;
    let outcome: String = row.try_get("outcome").map_err(db)?;
    Ok(Some(Stored {
        digest: digest32("digest")?,
        prev_digest: digest32("prev_digest")?,
        payee: crate::rows::account_id(row.try_get("payee_account_id").map_err(db)?)?,
        amount: Money::from_minor(
            row.try_get("amount_minor").map_err(db)?,
            Currency::from_code(&currency).map_err(storage)?,
        ),
        event_time: Timestamp::from_micros(row.try_get("event_micros").map_err(db)?),
        outcome: stored_outcome(&outcome)?,
        entry: entry.map(entry_id).transpose()?,
    }))
}

async fn insert_receipt(
    conn: &mut PgConnection,
    receipt: &Receipt,
    upload: &Upload<'_>,
    outcome: Outcome,
) -> Result<(), OfflineError> {
    sqlx::query(
        "INSERT INTO offline_receipts
             (device_id, counter, voucher_id, digest, prev_digest, body, signature,
              payee_account_id, currency, amount_minor, event_time, outcome, uploaded_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 timestamptz 'epoch' + $11::bigint * interval '1 microsecond', $12, $13)",
    )
    .bind(receipt.device_id.as_uuid())
    .bind(i64::try_from(receipt.counter).map_err(storage)?)
    .bind(receipt.voucher_id.as_uuid())
    .bind(&receipt.digest()[..])
    .bind(&receipt.prev_digest[..])
    .bind(upload.body)
    .bind(upload.signature)
    .bind(receipt.payee_account_id.as_uuid())
    .bind(receipt.amount.currency().code())
    .bind(receipt.amount.amount())
    .bind(receipt.event_time.as_micros())
    .bind(db_outcome(outcome))
    .bind(upload.client.as_uuid())
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(())
}

async fn set_outcome(
    conn: &mut PgConnection,
    device: DeviceId,
    counter: u64,
    outcome: Outcome,
) -> Result<(), OfflineError> {
    sqlx::query("UPDATE offline_receipts SET outcome = $3 WHERE device_id = $1 AND counter = $2")
        .bind(device.as_uuid())
        .bind(i64::try_from(counter).map_err(storage)?)
        .bind(db_outcome(outcome))
        .execute(&mut *conn)
        .await
        .map_err(sql)?;
    Ok(())
}

/// Freezes the device and its voucher at once (protocol §10), and records
/// why as an audit event: the alert for the client and the operator.
async fn freeze(
    conn: &mut PgConnection,
    device: DeviceId,
    client: ClientId,
    voucher: VoucherId,
    counter: u64,
    outcome: Outcome,
    now: Timestamp,
) -> Result<(), OfflineError> {
    let reason = format!("{} at counter {counter}", outcome.as_str());
    sqlx::query(
        "UPDATE devices SET status = 'frozen', status_reason = $2,
                status_changed_at = clock_timestamp()
          WHERE id = $1 AND status = 'active'",
    )
    .bind(device.as_uuid())
    .bind(&reason)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    sqlx::query(
        "UPDATE vouchers SET status = 'frozen', status_changed_at = clock_timestamp()
          WHERE id = $1 AND status IN ('active', 'grace')",
    )
    .bind(voucher.as_uuid())
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    let event = crate::tamper::NewAuditEvent {
        client: Some(client),
        actor: Some("ledger".into()),
        action: "offline.freeze".into(),
        target: Some(format!("device:{device}")),
        request_id: None,
        outcome: outcome.as_str().to_lowercase(),
        details: format!(
            r#"{{"voucher":"{voucher}","counter":{counter},"outcome":"{}"}}"#,
            outcome.as_str()
        ),
    };
    crate::tamper::append_audit(conn, &event, now).await?;
    Ok(())
}

/// The payee must be an open wallet (2010) or merchant balance (2030) of
/// the device's client.
async fn check_payee(
    conn: &mut PgConnection,
    payee: AccountId,
    client: ClientId,
) -> Result<Result<(), String>, LedgerError> {
    let row = sqlx::query("SELECT code, status, opened_by FROM accounts WHERE id = $1")
        .bind(payee.as_uuid())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db)?;
    let Some(row) = row else {
        return Ok(Err(format!("no payee account {payee}")));
    };
    let code: i16 = row.try_get("code").map_err(db)?;
    let status: String = row.try_get("status").map_err(db)?;
    let owner: Option<Uuid> = row.try_get("opened_by").map_err(db)?;
    Ok(if !matches!(code, 2010 | 2030) {
        Err(format!(
            "the payee account {payee} is not a wallet or a merchant balance"
        ))
    } else if owner != Some(client.as_uuid()) {
        Err(format!("no payee account {payee}"))
    } else if status != "open" {
        Err(format!("the payee account {payee} is not open"))
    } else {
        Ok(())
    })
}

/// What the next receipt of `voucher` after `counter` must link to: the
/// voucher's digest for its first counter, otherwise the previous receipt's
/// digest if it has settled, or `None` if it has not (a gap).
async fn expected_link(
    conn: &mut PgConnection,
    voucher: &IssuedVoucher,
    counter: u64,
) -> Result<Option<[u8; 32]>, LedgerError> {
    if counter == voucher.voucher.counter_start {
        return Ok(Some(voucher.voucher.digest()));
    }
    let previous = load_stored(conn, voucher.voucher.device_id, counter - 1).await?;
    Ok(previous
        .filter(|p| matches!(p.outcome, Outcome::Settled | Outcome::SettledLate))
        .map(|p| p.digest))
}

/// Settles one recorded receipt from escrow, or decides why not. Returns
/// its new outcome and, if settled, its entry.
#[allow(clippy::too_many_arguments)]
async fn try_settle<F>(
    conn: &mut PgConnection,
    voucher: &IssuedVoucher,
    counter: u64,
    stored: &Stored,
    fee_bps: u32,
    policy: FeePolicy,
    book: &F,
    now: Timestamp,
    options: &PostOptions,
) -> Result<(Outcome, Option<EntryId>), OfflineError>
where
    F: Fn(&BTreeMap<AccountId, Account>, &SettleRequest) -> Result<EntryDraft, String>,
{
    let device = voucher.voucher.device_id;
    // 8: the previous counter must have settled.
    let Some(link) = expected_link(conn, voucher, counter).await? else {
        return Ok((Outcome::WaitingForGap, None));
    };
    // 9: the chain link.
    if link != stored.prev_digest {
        return Ok((Outcome::Conflict, None));
    }
    // 10, 11: the escrow left.
    match voucher.status {
        VoucherStatus::Active | VoucherStatus::Grace => {}
        VoucherStatus::Frozen => return Ok((Outcome::Frozen, None)),
        VoucherStatus::Ended => return Ok((Outcome::Expired, None)),
        VoucherStatus::Returned | VoucherStatus::Closed => {
            return Err(OfflineError::NotYet(
                "receipts after a voucher's return settle from step 2.13".into(),
            ));
        }
    }
    let escrow_id = voucher.voucher.escrow_account_id;
    let escrow = crate::accounts::get_account(conn, escrow_id)
        .await?
        .ok_or(LedgerError::AccountNotFound(escrow_id))?;
    if stored.amount.amount() > escrow.balance().amount() {
        return Ok((Outcome::OverLimit, None));
    }

    // E17.
    let fee = fee_on(stored.amount, fee_bps);
    if fee.is_positive() && policy.fee_account.is_none() {
        return Err(OfflineError::Booking(
            "a fee is due but there is no fee account".into(),
        ));
    }
    let in_bounds = stored.event_time >= voucher.voucher.issued_at && stored.event_time <= now;
    let request = SettleRequest {
        escrow: escrow_id,
        payee: stored.payee,
        fee_account: policy.fee_account.filter(|_| fee.is_positive()),
        amount: stored.amount,
        fee,
        voucher: voucher.voucher.voucher_id,
        device,
        counter,
        event_time: in_bounds.then_some(stored.event_time),
        claimed_time: (!in_bounds).then_some(stored.event_time),
    };
    let mut view = BTreeMap::new();
    for id in [Some(escrow_id), Some(stored.payee), request.fee_account]
        .into_iter()
        .flatten()
    {
        let account = crate::accounts::get_account(conn, id)
            .await?
            .ok_or(LedgerError::AccountNotFound(id))?;
        view.insert(id, account);
    }
    let draft = book(&view, &request).map_err(OfflineError::Booking)?;
    check_booking(&draft, &request)?;

    let entry = EntryId::from_uuid(Uuid::now_v7()).map_err(storage)?;
    let key = IdempotencyKey::new(format!("offline:{device}:{counter}")).map_err(storage)?;
    let posting = PostRequest::new(entry, voucher.voucher.client_id, key, draft);
    let fingerprint = posting.draft.fingerprint();
    let entry = match claim_key(conn, &posting, &fingerprint).await? {
        // Unreachable while the receipt row is unsettled, but harmless: the
        // entry that paid it already exists.
        Claim::Existing(posted) => posted.id(),
        Claim::New => post_in_transaction(conn, posting, None, options)
            .await?
            .id(),
    };
    sqlx::query(
        "UPDATE offline_receipts
            SET outcome = 'settled', entry_id = $3, from_escrow_minor = amount_minor,
                settled_at = clock_timestamp()
          WHERE device_id = $1 AND counter = $2",
    )
    .bind(device.as_uuid())
    .bind(i64::try_from(counter).map_err(storage)?)
    .bind(entry.as_uuid())
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok((Outcome::Settled, Some(entry)))
}

/// Checks that `draft` is E17 for this receipt: the amount from escrow, the
/// amount less the fee to the payee, the fee to the fee account.
fn check_booking(draft: &EntryDraft, request: &SettleRequest) -> Result<(), OfflineError> {
    let mut expected = vec![
        (request.escrow, Side::Debit, request.amount),
        (
            request.payee,
            Side::Credit,
            Money::from_minor(
                request.amount.amount() - request.fee.amount(),
                request.amount.currency(),
            ),
        ),
    ];
    if let Some(fee_account) = request.fee_account {
        expected.push((fee_account, Side::Credit, request.fee));
    }
    let mut lines: Vec<(AccountId, Side, Money)> = draft
        .lines
        .iter()
        .map(|l| (l.account, l.side, l.amount))
        .collect();
    let key = |l: &(AccountId, Side, Money)| (l.0, l.1 == Side::Debit);
    lines.sort_by_key(key);
    expected.sort_by_key(key);
    if lines != expected {
        return Err(OfflineError::Booking(
            "the booking does not pay the receipt from its escrow".into(),
        ));
    }
    let named = |key: &str, value: String| draft.metadata.get(key) == Some(&value);
    if !(named("voucher", request.voucher.to_string())
        && named("device", request.device.to_string())
        && named("counter", request.counter.to_string()))
    {
        return Err(OfflineError::Booking(
            "the booking does not name the receipt".into(),
        ));
    }
    Ok(())
}

async fn lock_voucher(
    conn: &mut PgConnection,
    device: DeviceId,
    voucher: VoucherId,
) -> Result<Option<IssuedVoucher>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {VOUCHER_COLUMNS} FROM vouchers WHERE id = $1 AND device_id = $2 FOR UPDATE"
    )))
    .bind(voucher.as_uuid())
    .bind(device.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(voucher_from_row).transpose()
}

#[allow(clippy::too_many_lines)]
async fn ingest_in<F>(
    conn: &mut PgConnection,
    receipt: &Receipt,
    upload: &Upload<'_>,
    policy: FeePolicy,
    book: &F,
    options: &PostOptions,
) -> Result<IngestResult, OfflineError>
where
    F: Fn(&BTreeMap<AccountId, Account>, &SettleRequest) -> Result<EntryDraft, String>,
{
    let refused = |o: Outcome, why: String| Ok(IngestResult::refused(o, Some(receipt), why));
    // 2: the device, locked: everything for one device queues here.
    let device = match load_device(conn, Some(upload.client), receipt.device_id, true).await {
        Ok(device) => device,
        Err(OfflineError::DeviceNotFound) => {
            return refused(Outcome::UnknownDevice, "no such device".into());
        }
        Err(other) => return Err(other),
    };
    // 3: the signature, before anything is written.
    if receipt
        .verify(&device.certificate.public_key, upload.signature)
        .is_err()
    {
        return refused(
            Outcome::BadSignature,
            "the signature does not verify".into(),
        );
    }
    // 4: the voucher.
    let Some(voucher) = lock_voucher(conn, receipt.device_id, receipt.voucher_id).await? else {
        return refused(Outcome::UnknownVoucher, "not this device's voucher".into());
    };
    if receipt.counter < voucher.voucher.counter_start {
        return refused(
            Outcome::UnknownVoucher,
            format!(
                "counter {} is before the voucher's first, {}",
                receipt.counter, voucher.voucher.counter_start
            ),
        );
    }
    if receipt.amount.currency() != voucher.voucher.amount.currency()
        || !receipt.amount.is_positive()
    {
        return refused(
            Outcome::BadFormat,
            "the amount is not a positive amount in the voucher's currency".into(),
        );
    }
    let client = device.certificate.client_id;
    if let Err(why) = check_payee(conn, receipt.payee_account_id, client).await? {
        return refused(Outcome::BadFormat, why);
    }

    let digest = receipt.digest();
    let mut result = IngestResult {
        outcome: Outcome::Duplicate,
        device: Some(receipt.device_id),
        counter: Some(receipt.counter),
        digest: Some(digest),
        entry: None,
        detail: None,
        settled: Vec::new(),
    };
    // 5, 6: already recorded?
    if let Some(stored) = load_stored(conn, receipt.device_id, receipt.counter).await? {
        if stored.digest == digest {
            result.entry = stored.entry;
            result.detail = Some(format!("recorded as {}", stored.outcome.as_str()));
            return Ok(result);
        }
        sqlx::query(
            "INSERT INTO offline_receipt_conflicts
                 (device_id, counter, digest, body, signature, uploaded_by)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (device_id, counter, digest) DO NOTHING",
        )
        .bind(receipt.device_id.as_uuid())
        .bind(i64::try_from(receipt.counter).map_err(storage)?)
        .bind(&digest[..])
        .bind(upload.body)
        .bind(upload.signature)
        .bind(upload.client.as_uuid())
        .execute(&mut *conn)
        .await
        .map_err(sql)?;
        freeze(
            conn,
            receipt.device_id,
            client,
            receipt.voucher_id,
            receipt.counter,
            Outcome::Conflict,
            upload.now,
        )
        .await?;
        result.outcome = Outcome::Conflict;
        result.detail = Some(format!(
            "counter {} is recorded with another receipt",
            receipt.counter
        ));
        return Ok(result);
    }

    // Recorded now, whatever happens next.
    // 7: frozen?
    let frozen = device.status == DeviceStatus::Frozen || voucher.status == VoucherStatus::Frozen;
    insert_receipt(
        conn,
        receipt,
        upload,
        if frozen {
            Outcome::Frozen
        } else {
            Outcome::WaitingForGap
        },
    )
    .await?;
    if frozen {
        result.outcome = Outcome::Frozen;
        result.detail = Some("the device or its voucher is frozen".into());
        return Ok(result);
    }

    // 8–11, then whatever was waiting for it, in counter order.
    let fee_bps = offline_fee_bps(conn, client).await?;
    let mut counter = receipt.counter;
    loop {
        let Some(stored) = load_stored(conn, receipt.device_id, counter).await? else {
            break;
        };
        let (outcome, entry) = try_settle(
            conn, &voucher, counter, &stored, fee_bps, policy, book, upload.now, options,
        )
        .await?;
        let uploaded = counter == receipt.counter;
        if uploaded {
            result.outcome = outcome;
            result.entry = entry;
        }
        match (outcome, entry) {
            (Outcome::Settled, Some(entry)) => {
                result.settled.push(Settled { counter, entry });
                counter += 1;
                // Only a receipt that was waiting continues the chain.
                match load_stored(conn, receipt.device_id, counter).await? {
                    Some(next) if next.outcome == Outcome::WaitingForGap => continue,
                    _ => break,
                }
            }
            (Outcome::WaitingForGap, _) => break,
            (Outcome::Conflict | Outcome::OverLimit, _) => {
                set_outcome(conn, receipt.device_id, counter, outcome).await?;
                freeze(
                    conn,
                    receipt.device_id,
                    client,
                    receipt.voucher_id,
                    counter,
                    outcome,
                    upload.now,
                )
                .await?;
                if uploaded {
                    result.detail = Some(match outcome {
                        Outcome::Conflict => "the chain link does not match".into(),
                        _ => "more than the escrow left".into(),
                    });
                }
                break;
            }
            (other, _) => {
                set_outcome(conn, receipt.device_id, counter, other).await?;
                break;
            }
        }
    }
    Ok(result)
}

/// Ingests one uploaded receipt (protocol §9): see the module
/// documentation. One transaction, retried on contention; a refusal writes
/// nothing.
pub async fn ingest_receipt<F>(
    conn: &mut PgConnection,
    upload: &Upload<'_>,
    policy: FeePolicy,
    book: F,
    options: &PostOptions,
) -> Result<IngestResult, OfflineError>
where
    F: Fn(&BTreeMap<AccountId, Account>, &SettleRequest) -> Result<EntryDraft, String>,
{
    // 1: decode.
    let receipt = match Receipt::decode(upload.body) {
        Ok(receipt) => receipt,
        Err(e) => {
            return Ok(IngestResult::refused(
                Outcome::BadFormat,
                None,
                e.to_string(),
            ));
        }
    };
    if upload.signature.len() != mezgeb_offline::crypto::SIGNATURE_LEN {
        return Ok(IngestResult::refused(
            Outcome::BadSignature,
            Some(&receipt),
            "a signature is 64 bytes",
        ));
    }
    let mut attempt = 1;
    loop {
        let result = async {
            let mut tx = options.begin(conn).await?;
            match ingest_in(&mut tx, &receipt, upload, policy, &book, options).await {
                // Refusals write nothing: roll back what was locked.
                Ok(result) if is_refusal(result.outcome) => {
                    let _ = tx.rollback().await;
                    Ok(result)
                }
                Ok(result) => {
                    options.commit(tx).await?;
                    Ok(result)
                }
                Err(error) => {
                    let _ = tx.rollback().await;
                    Err(error)
                }
            }
        }
        .await;
        match result {
            Err(OfflineError::Ledger(error)) if options.retry_after(&error, attempt).await => {
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// The outcomes that record nothing.
fn is_refusal(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::BadFormat
            | Outcome::BadSignature
            | Outcome::UnknownDevice
            | Outcome::UnknownVoucher
            | Outcome::Duplicate
    )
}

/// A recorded receipt, for the API and the dispute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRecord {
    pub device: DeviceId,
    pub counter: u64,
    pub voucher: VoucherId,
    pub digest: [u8; 32],
    pub payee: AccountId,
    pub amount: Money,
    pub event_time: Timestamp,
    pub outcome: Outcome,
    pub entry: Option<EntryId>,
}

/// The receipts recorded for a voucher of `client`, in counter order.
pub async fn voucher_receipts(
    conn: &mut PgConnection,
    client: ClientId,
    voucher: VoucherId,
) -> Result<Vec<ReceiptRecord>, OfflineError> {
    let rows = sqlx::query(
        "SELECT r.device_id, r.counter, r.digest, r.payee_account_id, r.currency,
                r.amount_minor, r.outcome, r.entry_id,
                (extract(epoch FROM r.event_time) * 1000000)::bigint AS event_micros
           FROM offline_receipts r JOIN vouchers v ON v.id = r.voucher_id
          WHERE r.voucher_id = $1 AND v.client_id = $2
          ORDER BY r.counter",
    )
    .bind(voucher.as_uuid())
    .bind(client.as_uuid())
    .fetch_all(&mut *conn)
    .await
    .map_err(sql)?;
    rows.iter()
        .map(|row| -> Result<ReceiptRecord, LedgerError> {
            let digest: Vec<u8> = row.try_get("digest").map_err(db)?;
            let counter: i64 = row.try_get("counter").map_err(db)?;
            let currency: String = row.try_get("currency").map_err(db)?;
            let outcome: String = row.try_get("outcome").map_err(db)?;
            let entry: Option<Uuid> = row.try_get("entry_id").map_err(db)?;
            Ok(ReceiptRecord {
                device: DeviceId::from_uuid(row.try_get("device_id").map_err(db)?)
                    .map_err(storage)?,
                counter: u64::try_from(counter).map_err(storage)?,
                voucher,
                digest: digest
                    .try_into()
                    .map_err(|_| storage("a digest is not 32 bytes"))?,
                payee: crate::rows::account_id(row.try_get("payee_account_id").map_err(db)?)?,
                amount: Money::from_minor(
                    row.try_get("amount_minor").map_err(db)?,
                    Currency::from_code(&currency).map_err(storage)?,
                ),
                event_time: Timestamp::from_micros(row.try_get("event_micros").map_err(db)?),
                outcome: stored_outcome(&outcome)?,
                entry: entry.map(entry_id).transpose()?,
            })
        })
        .collect::<Result<_, _>>()
        .map_err(OfflineError::from)
}

/// The digest in hex, for messages and the API.
pub fn digest_hex(digest: &[u8; 32]) -> String {
    hex(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fee_rounds_down_in_the_payees_favour() {
        let etb = |m| Money::from_minor(m, Currency::ETB);
        assert_eq!(fee_on(etb(6_000), 200), etb(120));
        assert_eq!(fee_on(etb(99), 200), etb(1));
        assert_eq!(fee_on(etb(49), 200), etb(0));
        assert_eq!(fee_on(etb(6_000), 0), etb(0));
    }

    #[test]
    fn stored_outcomes_round_trip() {
        for o in [
            Outcome::Settled,
            Outcome::SettledLate,
            Outcome::WaitingForGap,
            Outcome::Conflict,
            Outcome::Frozen,
            Outcome::OverLimit,
            Outcome::Expired,
        ] {
            assert_eq!(stored_outcome(&db_outcome(o)), Ok(o));
        }
    }
}

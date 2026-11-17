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
//! After the voucher's return (step 2.13), a receipt is paid by the loss
//! policy: the payer's wallet first, then the loss account 5030, and the
//! shortfall is recorded against the payer (`SETTLED_LATE`). After the late
//! deadline it is `EXPIRED`. A receipt whose successor already settled past
//! a skipped gap must be the one the successor committed to (§11).
//!
//! The engine knows nothing about templates: the caller books every entry
//! through [`OfflineBooks`], and the engine checks each draft moves exactly
//! what the decision says.

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

/// The ledger's accounts that offline payments use, found by the caller
/// from the chart.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfflineAccounts {
    /// 4010 Payment fee revenue. Needed only when a fee is due.
    pub fee_account: Option<AccountId>,
    /// 5030 Offline settlement losses. Needed only when a late receipt's
    /// payer cannot pay it all.
    pub loss_account: Option<AccountId>,
}

/// What `settle` is asked to draft: E17 for one receipt.
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

/// What `pay` is asked to draft: a receipt paid by the loss policy, from
/// escrow, the payer's wallet and the loss account, each only if its part
/// is positive (E26, E28, E29, E31).
#[derive(Debug, Clone)]
pub struct PayRequest {
    pub escrow: Option<AccountId>,
    pub wallet: Option<AccountId>,
    pub loss_account: Option<AccountId>,
    pub payee: AccountId,
    pub fee_account: Option<AccountId>,
    pub from_escrow: Money,
    pub from_wallet: Money,
    pub from_loss: Money,
    /// The merchant fee, possibly zero.
    pub fee: Money,
    pub voucher: VoucherId,
    pub device: DeviceId,
    pub counter: u64,
    pub event_time: Option<Timestamp>,
    pub claimed_time: Option<Timestamp>,
}

/// What `return_escrow` is asked to draft: the whole escrow left back to
/// the wallet (E18, E25, E27).
#[derive(Debug, Clone)]
pub struct ReturnRequest {
    pub escrow: AccountId,
    pub wallet: AccountId,
    pub amount: Money,
    pub voucher: VoucherId,
    /// `expired`, `closed` or `dispute`.
    pub reason: &'static str,
}

/// What `repay` is asked to draft: a shortfall repaid from the wallet to
/// the loss account (E30).
#[derive(Debug, Clone)]
pub struct RepayRequest {
    pub wallet: AccountId,
    pub loss_account: AccountId,
    pub amount: Money,
    pub shortfall: Uuid,
}

/// How the offline entries are booked. The engine knows nothing about
/// templates: the caller implements this with them, and the engine checks
/// every draft moves exactly what the decision says.
pub trait OfflineBooks: Sync {
    /// E17.
    fn settle(
        &self,
        view: &BTreeMap<AccountId, Account>,
        request: &SettleRequest,
    ) -> Result<EntryDraft, String>;
    /// E26, E28, E29, E31.
    fn pay(
        &self,
        view: &BTreeMap<AccountId, Account>,
        request: &PayRequest,
    ) -> Result<EntryDraft, String>;
    /// E18, E25, E27.
    fn return_escrow(
        &self,
        view: &BTreeMap<AccountId, Account>,
        request: &ReturnRequest,
    ) -> Result<EntryDraft, String>;
    /// E30.
    fn repay(
        &self,
        view: &BTreeMap<AccountId, Account>,
        request: &RepayRequest,
    ) -> Result<EntryDraft, String>;
}

/// What a decision needs besides the database.
pub(crate) struct Ctx<'a, B: ?Sized> {
    pub books: &'a B,
    pub accounts: OfflineAccounts,
    /// Ledger time.
    pub now: Timestamp,
    pub options: &'a PostOptions,
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

/// Sets how many days after a voucher's return a late receipt is still
/// paid (protocol §12): 1 to 365, 90 without a setting. For `mezgeb-cli
/// offline deadline`, as `mezgeb_admin`. A new deadline applies to every
/// returned voucher from the next tick.
pub async fn set_late_deadline(
    conn: &mut PgConnection,
    client: ClientId,
    days: u32,
) -> Result<(), OfflineError> {
    let days = i32::try_from(days)
        .ok()
        .filter(|d| (1..=365).contains(d))
        .ok_or_else(|| OfflineError::Invalid("the late deadline is 1 to 365 days".into()))?;
    sqlx::query(
        "INSERT INTO offline_settings (client_id, late_deadline_days) VALUES ($1, $2)
         ON CONFLICT (client_id) DO UPDATE
            SET late_deadline_days = EXCLUDED.late_deadline_days,
                updated_at = clock_timestamp()",
    )
    .bind(client.as_uuid())
    .bind(days)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(())
}

/// The fee on `amount` at `bps`, rounded down: a fraction of a santim is
/// the payee's.
pub(crate) fn fee_on(amount: Money, bps: u32) -> Money {
    let minor = i128::from(amount.amount()) * i128::from(bps) / 10_000;
    Money::from_minor(i64::try_from(minor).unwrap_or(0), amount.currency())
}

/// A stored receipt, as the checks need it.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub digest: [u8; 32],
    pub prev_digest: [u8; 32],
    pub payee: AccountId,
    pub amount: Money,
    pub event_time: Timestamp,
    pub outcome: Outcome,
    pub entry: Option<EntryId>,
}

pub(crate) fn stored_outcome(text: &str) -> Result<Outcome, LedgerError> {
    Outcome::parse(&text.to_uppercase()).ok_or_else(|| storage(format!("outcome {text:?}")))
}

pub(crate) fn db_outcome(outcome: Outcome) -> String {
    outcome.as_str().to_lowercase()
}

pub(crate) async fn load_stored(
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
              payee_account_id, currency, amount_minor, event_time, outcome, uploaded_by,
              received_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 timestamptz 'epoch' + $11::bigint * interval '1 microsecond', $12, $13,
                 timestamptz 'epoch' + $14::bigint * interval '1 microsecond')",
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
    // Ledger time, as every other offline deadline: the gap timeout is
    // measured from it (protocol §11).
    .bind(upload.now.as_micros())
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(())
}

pub(crate) async fn set_outcome(
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
pub(crate) async fn freeze(
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
    // The case for the operator (protocol §14): one open case per voucher.
    sqlx::query(
        "INSERT INTO offline_disputes (id, client_id, device_id, voucher_id, reason, counter)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (voucher_id) WHERE status = 'open' DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(client.as_uuid())
    .bind(device.as_uuid())
    .bind(voucher.as_uuid())
    .bind(outcome.as_str().to_lowercase())
    .bind(i64::try_from(counter).map_err(storage)?)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
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
    Ok(previous.filter(|p| is_paid(p.outcome)).map(|p| p.digest))
}

pub(crate) fn is_paid(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Settled | Outcome::SettledLate)
}

/// The accounts of `ids`, as a template sees them.
pub(crate) async fn view(
    conn: &mut PgConnection,
    ids: &[AccountId],
) -> Result<BTreeMap<AccountId, Account>, LedgerError> {
    let mut view = BTreeMap::new();
    for id in ids {
        let account = crate::accounts::get_account(conn, *id)
            .await?
            .ok_or(LedgerError::AccountNotFound(*id))?;
        view.insert(*id, account);
    }
    Ok(view)
}

/// An account's balance on its normal side, in minor units.
pub(crate) async fn balance_of(conn: &mut PgConnection, id: AccountId) -> Result<i64, LedgerError> {
    Ok(crate::accounts::get_account(conn, id)
        .await?
        .ok_or(LedgerError::AccountNotFound(id))?
        .balance()
        .amount())
}

/// Posts one of the offline entries inside the open transaction, under its
/// own idempotency key.
pub(crate) async fn post_offline(
    conn: &mut PgConnection,
    client: ClientId,
    key: &str,
    draft: EntryDraft,
    options: &PostOptions,
) -> Result<EntryId, OfflineError> {
    let entry = EntryId::from_uuid(Uuid::now_v7()).map_err(storage)?;
    let key = IdempotencyKey::new(key).map_err(storage)?;
    let posting = PostRequest::new(entry, client, key, draft);
    let fingerprint = posting.draft.fingerprint();
    Ok(match claim_key(conn, &posting, &fingerprint).await? {
        Claim::Existing(posted) => posted.id(),
        Claim::New => post_in_transaction(conn, posting, None, options)
            .await?
            .id(),
    })
}

/// Whether `draft` has exactly these lines, in any order.
pub(crate) fn has_lines(draft: &EntryDraft, mut expected: Vec<(AccountId, Side, Money)>) -> bool {
    let mut lines: Vec<(AccountId, Side, Money)> = draft
        .lines
        .iter()
        .map(|l| (l.account, l.side, l.amount))
        .collect();
    let key = |l: &(AccountId, Side, Money)| (l.0, l.1 == Side::Debit);
    lines.sort_by_key(key);
    expected.sort_by_key(key);
    lines == expected
}

/// Whether `draft` names each of these metadata values.
pub(crate) fn names(draft: &EntryDraft, pairs: &[(&str, String)]) -> bool {
    pairs
        .iter()
        .all(|(key, value)| draft.metadata.get(*key) == Some(value))
}

fn net(amount: Money, fee: Money) -> Money {
    Money::from_minor(amount.amount() - fee.amount(), amount.currency())
}

/// Checks that `draft` is E17 for this receipt: the amount from escrow, the
/// amount less the fee to the payee, the fee to the fee account.
fn check_booking(draft: &EntryDraft, request: &SettleRequest) -> Result<(), OfflineError> {
    let mut expected = vec![
        (request.escrow, Side::Debit, request.amount),
        (
            request.payee,
            Side::Credit,
            net(request.amount, request.fee),
        ),
    ];
    if let Some(fee_account) = request.fee_account {
        expected.push((fee_account, Side::Credit, request.fee));
    }
    if !has_lines(draft, expected) {
        return Err(OfflineError::Booking(
            "the booking does not pay the receipt from its escrow".into(),
        ));
    }
    if !names(
        draft,
        &[
            ("voucher", request.voucher.to_string()),
            ("device", request.device.to_string()),
            ("counter", request.counter.to_string()),
        ],
    ) {
        return Err(OfflineError::Booking(
            "the booking does not name the receipt".into(),
        ));
    }
    Ok(())
}

/// The device's time, if it lies between the voucher's issue and now.
fn bounded_time(voucher: &IssuedVoucher, time: Timestamp, now: Timestamp) -> Option<Timestamp> {
    (time >= voucher.voucher.issued_at && time <= now).then_some(time)
}

/// E17: pays a stored receipt from escrow, and records it settled.
async fn settle_from_escrow<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    ctx: &Ctx<'_, B>,
    voucher: &IssuedVoucher,
    counter: u64,
    stored: &Stored,
    gap_skipped: bool,
) -> Result<EntryId, OfflineError> {
    let device = voucher.voucher.device_id;
    let fee = fee_on(
        stored.amount,
        offline_fee_bps(conn, voucher.voucher.client_id).await?,
    );
    if fee.is_positive() && ctx.accounts.fee_account.is_none() {
        return Err(OfflineError::Booking(
            "a fee is due but there is no fee account".into(),
        ));
    }
    let event_time = bounded_time(voucher, stored.event_time, ctx.now);
    let request = SettleRequest {
        escrow: voucher.voucher.escrow_account_id,
        payee: stored.payee,
        fee_account: ctx.accounts.fee_account.filter(|_| fee.is_positive()),
        amount: stored.amount,
        fee,
        voucher: voucher.voucher.voucher_id,
        device,
        counter,
        event_time,
        claimed_time: event_time.is_none().then_some(stored.event_time),
    };
    let ids: Vec<AccountId> = [
        Some(request.escrow),
        Some(request.payee),
        request.fee_account,
    ]
    .into_iter()
    .flatten()
    .collect();
    let draft = ctx
        .books
        .settle(&view(conn, &ids).await?, &request)
        .map_err(OfflineError::Booking)?;
    check_booking(&draft, &request)?;
    let entry = post_offline(
        conn,
        voucher.voucher.client_id,
        &format!("offline:{device}:{counter}"),
        draft,
        ctx.options,
    )
    .await?;
    record_paid(
        conn,
        device,
        counter,
        Outcome::Settled,
        entry,
        Parts {
            escrow: stored.amount.amount(),
            wallet: 0,
            loss: 0,
        },
        gap_skipped,
    )
    .await?;
    Ok(entry)
}

/// How much of a payment came from each source, in minor units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Parts {
    pub escrow: i64,
    pub wallet: i64,
    pub loss: i64,
}

/// One receipt to pay under the loss policy: a late receipt, or one a
/// dispute pays.
pub(crate) struct ToPay {
    pub device: DeviceId,
    pub counter: u64,
    pub payee: AccountId,
    pub amount: Money,
    pub event_time: Timestamp,
    /// The entry's idempotency key.
    pub key: String,
    /// Whether the voucher's escrow pays first (a dispute), or not (a late
    /// receipt, after the escrow went back).
    pub use_escrow: bool,
}

/// Pays a receipt from escrow (if `use_escrow`) as far as it goes, then the
/// payer's wallet as far as its balance allows, then the loss account
/// (ADR-0019, protocol §12; E26, E28, E29, E31). Records a shortfall for
/// what the loss account paid. Returns the entry and the parts.
pub(crate) async fn pay_receipt<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    ctx: &Ctx<'_, B>,
    voucher: &IssuedVoucher,
    pay: &ToPay,
) -> Result<(EntryId, Parts), OfflineError> {
    let client = voucher.voucher.client_id;
    let amount = pay.amount.amount();
    let escrow = voucher.voucher.escrow_account_id;
    let wallet = voucher.owner_account;
    // The wallet, locked before its balance is read, so the part it pays
    // cannot change before the entry posts.
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR NO KEY UPDATE")
        .bind(wallet.as_uuid())
        .fetch_optional(&mut *conn)
        .await
        .map_err(sql)?;
    let from_escrow = if pay.use_escrow {
        balance_of(conn, escrow).await?.clamp(0, amount)
    } else {
        0
    };
    let from_wallet = balance_of(conn, wallet)
        .await?
        .clamp(0, amount - from_escrow);
    let from_loss = amount - from_escrow - from_wallet;
    let loss_account = ctx.accounts.loss_account;
    if from_loss > 0 && loss_account.is_none() {
        return Err(OfflineError::Booking(
            "the loss account must pay part of it but there is none".into(),
        ));
    }
    let fee = fee_on(pay.amount, offline_fee_bps(conn, client).await?);
    if fee.is_positive() && ctx.accounts.fee_account.is_none() {
        return Err(OfflineError::Booking(
            "a fee is due but there is no fee account".into(),
        ));
    }
    let currency = pay.amount.currency();
    let money = |minor| Money::from_minor(minor, currency);
    let event_time = bounded_time(voucher, pay.event_time, ctx.now);
    let request = PayRequest {
        escrow: (from_escrow > 0).then_some(escrow),
        wallet: (from_wallet > 0).then_some(wallet),
        loss_account: loss_account.filter(|_| from_loss > 0),
        payee: pay.payee,
        fee_account: ctx.accounts.fee_account.filter(|_| fee.is_positive()),
        from_escrow: money(from_escrow),
        from_wallet: money(from_wallet),
        from_loss: money(from_loss),
        fee,
        voucher: voucher.voucher.voucher_id,
        device: pay.device,
        counter: pay.counter,
        event_time,
        claimed_time: event_time.is_none().then_some(pay.event_time),
    };
    let ids: Vec<AccountId> = [
        request.escrow,
        request.wallet,
        request.loss_account,
        Some(request.payee),
        request.fee_account,
    ]
    .into_iter()
    .flatten()
    .collect();
    let draft = ctx
        .books
        .pay(&view(conn, &ids).await?, &request)
        .map_err(OfflineError::Booking)?;
    let mut expected = vec![(request.payee, Side::Credit, net(pay.amount, fee))];
    for (account, part) in [
        (request.escrow, request.from_escrow),
        (request.wallet, request.from_wallet),
        (request.loss_account, request.from_loss),
    ] {
        if let Some(account) = account {
            expected.push((account, Side::Debit, part));
        }
    }
    if let Some(fee_account) = request.fee_account {
        expected.push((fee_account, Side::Credit, fee));
    }
    if !has_lines(&draft, expected)
        || !names(
            &draft,
            &[
                ("voucher", request.voucher.to_string()),
                ("device", request.device.to_string()),
                ("counter", request.counter.to_string()),
            ],
        )
    {
        return Err(OfflineError::Booking(
            "the booking does not pay the receipt by the loss policy".into(),
        ));
    }
    let entry = post_offline(conn, client, &pay.key, draft, ctx.options).await?;
    if from_loss > 0 {
        sqlx::query(
            "INSERT INTO offline_shortfalls
                 (id, client_id, owner_account_id, device_id, counter, currency, amount_minor,
                  entry_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(client.as_uuid())
        .bind(wallet.as_uuid())
        .bind(pay.device.as_uuid())
        .bind(i64::try_from(pay.counter).map_err(storage)?)
        .bind(currency.code())
        .bind(from_loss)
        .bind(entry.as_uuid())
        .execute(&mut *conn)
        .await
        .map_err(sql)?;
    }
    Ok((
        entry,
        Parts {
            escrow: from_escrow,
            wallet: from_wallet,
            loss: from_loss,
        },
    ))
}

/// Records a stored receipt as paid.
pub(crate) async fn record_paid(
    conn: &mut PgConnection,
    device: DeviceId,
    counter: u64,
    outcome: Outcome,
    entry: EntryId,
    parts: Parts,
    gap_skipped: bool,
) -> Result<(), OfflineError> {
    sqlx::query(
        "UPDATE offline_receipts
            SET outcome = $3, entry_id = $4, from_escrow_minor = $5, from_wallet_minor = $6,
                from_loss_minor = $7, gap_skipped = $8, settled_at = clock_timestamp()
          WHERE device_id = $1 AND counter = $2",
    )
    .bind(device.as_uuid())
    .bind(i64::try_from(counter).map_err(storage)?)
    .bind(db_outcome(outcome))
    .bind(entry.as_uuid())
    .bind(parts.escrow)
    .bind(parts.wallet)
    .bind(parts.loss)
    .bind(gap_skipped)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(())
}

/// Decides one recorded, unsettled receipt (protocol §9 checks 7–13, §11,
/// §12), and pays it if it may be paid. `skip_gap` lets it settle past a
/// missing predecessor: at the gap timeout, or at the voucher's return.
pub(crate) async fn decide<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    ctx: &Ctx<'_, B>,
    voucher: &IssuedVoucher,
    counter: u64,
    stored: &Stored,
    skip_gap: bool,
) -> Result<(Outcome, Option<EntryId>), OfflineError> {
    let device = voucher.voucher.device_id;
    match voucher.status {
        VoucherStatus::Frozen => return Ok((Outcome::Frozen, None)),
        VoucherStatus::Ended => return Ok((Outcome::Expired, None)),
        _ => {}
    }
    // A missing receipt arriving after its successor settled must be the
    // one the successor committed to (§11).
    let successor = load_stored(conn, device, counter + 1).await?;
    let successor_paid = successor.as_ref().is_some_and(|s| is_paid(s.outcome));
    if let Some(next) = successor.as_ref().filter(|s| is_paid(s.outcome))
        && next.prev_digest != stored.digest
    {
        return Ok((Outcome::Conflict, None));
    }
    // 8, 9: the link to the predecessor.
    let late = matches!(
        voucher.status,
        VoucherStatus::Returned | VoucherStatus::Closed
    );
    let mut gap_skipped = false;
    match expected_link(conn, voucher, counter).await? {
        Some(link) if link != stored.prev_digest => return Ok((Outcome::Conflict, None)),
        Some(_) => {}
        None if successor_paid || skip_gap || late => gap_skipped = true,
        None => return Ok((Outcome::WaitingForGap, None)),
    }
    if late {
        // 12: after the return, by the loss policy.
        let (entry, parts) = pay_receipt(
            conn,
            ctx,
            voucher,
            &ToPay {
                device,
                counter,
                payee: stored.payee,
                amount: stored.amount,
                event_time: stored.event_time,
                key: format!("offline:{device}:{counter}"),
                use_escrow: false,
            },
        )
        .await?;
        record_paid(
            conn,
            device,
            counter,
            Outcome::SettledLate,
            entry,
            parts,
            gap_skipped,
        )
        .await?;
        return Ok((Outcome::SettledLate, Some(entry)));
    }
    // 10, 11: the escrow left.
    let left = balance_of(conn, voucher.voucher.escrow_account_id).await?;
    if stored.amount.amount() > left {
        return Ok((Outcome::OverLimit, None));
    }
    let entry = settle_from_escrow(conn, ctx, voucher, counter, stored, gap_skipped).await?;
    Ok((Outcome::Settled, Some(entry)))
}

/// Applies a decision that pays nothing: a conflict or an over-spend
/// freezes; anything else is recorded as it is.
pub(crate) async fn apply_unpaid(
    conn: &mut PgConnection,
    voucher: &IssuedVoucher,
    counter: u64,
    outcome: Outcome,
    now: Timestamp,
) -> Result<(), OfflineError> {
    let device = voucher.voucher.device_id;
    if outcome != Outcome::WaitingForGap {
        set_outcome(conn, device, counter, outcome).await?;
    }
    if matches!(outcome, Outcome::Conflict | Outcome::OverLimit) {
        freeze(
            conn,
            device,
            voucher.voucher.client_id,
            voucher.voucher.voucher_id,
            counter,
            outcome,
            now,
        )
        .await?;
    }
    Ok(())
}

/// Settles the receipts of `voucher` that are waiting, in counter order,
/// starting at `from`, as far as they go: each one that settles lets the
/// next settle. With `skip_gap`, missing predecessors are skipped (§11).
/// Stops at a freeze. Returns what it settled.
pub(crate) async fn settle_waiting<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    ctx: &Ctx<'_, B>,
    voucher: &IssuedVoucher,
    skip_gap: bool,
) -> Result<Vec<Settled>, OfflineError> {
    let mut settled = Vec::new();
    loop {
        let next: Option<i64> = sqlx::query_scalar(
            "SELECT min(counter) FROM offline_receipts
              WHERE voucher_id = $1 AND outcome = 'waiting_for_gap'",
        )
        .bind(voucher.voucher.voucher_id.as_uuid())
        .fetch_one(&mut *conn)
        .await
        .map_err(sql)?;
        let Some(counter) = next.map(u64::try_from).transpose().map_err(storage)? else {
            break;
        };
        let stored = load_stored(conn, voucher.voucher.device_id, counter)
            .await?
            .ok_or_else(|| storage("a waiting receipt disappeared"))?;
        match decide(conn, ctx, voucher, counter, &stored, skip_gap).await? {
            (outcome, Some(entry)) if is_paid(outcome) => settled.push(Settled { counter, entry }),
            (Outcome::WaitingForGap, _) => break,
            (outcome, _) => {
                apply_unpaid(conn, voucher, counter, outcome, ctx.now).await?;
                break;
            }
        }
    }
    Ok(settled)
}

pub(crate) async fn lock_voucher(
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
async fn ingest_in<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    receipt: &Receipt,
    upload: &Upload<'_>,
    ctx: &Ctx<'_, B>,
) -> Result<IngestResult, OfflineError> {
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
                 (device_id, counter, digest, body, signature, uploaded_by, received_at)
             VALUES ($1, $2, $3, $4, $5, $6,
                     timestamptz 'epoch' + $7::bigint * interval '1 microsecond')
             ON CONFLICT (device_id, counter, digest) DO NOTHING",
        )
        .bind(receipt.device_id.as_uuid())
        .bind(i64::try_from(receipt.counter).map_err(storage)?)
        .bind(&digest[..])
        .bind(upload.body)
        .bind(upload.signature)
        .bind(upload.client.as_uuid())
        .bind(upload.now.as_micros())
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

    // 8–13 for this receipt, then whatever was waiting for it.
    let stored = load_stored(conn, receipt.device_id, receipt.counter)
        .await?
        .ok_or_else(|| storage("a receipt just recorded disappeared"))?;
    let (outcome, entry) = decide(conn, ctx, &voucher, receipt.counter, &stored, false).await?;
    result.outcome = outcome;
    result.entry = entry;
    match (outcome, entry) {
        (outcome, Some(entry)) if is_paid(outcome) => {
            result.settled.push(Settled {
                counter: receipt.counter,
                entry,
            });
            let next = load_stored(conn, receipt.device_id, receipt.counter + 1).await?;
            if next.is_some_and(|n| n.outcome == Outcome::WaitingForGap) {
                result
                    .settled
                    .extend(settle_waiting(conn, ctx, &voucher, false).await?);
            }
        }
        (outcome, _) => {
            apply_unpaid(conn, &voucher, receipt.counter, outcome, upload.now).await?;
            result.detail = match outcome {
                Outcome::Conflict => Some("the chain link does not match".into()),
                Outcome::OverLimit => Some("more than the escrow left".into()),
                Outcome::Expired => Some("past the voucher's late deadline".into()),
                _ => None,
            };
        }
    }
    Ok(result)
}

/// Ingests one uploaded receipt (protocol §9): see the module
/// documentation. One transaction, retried on contention; a refusal writes
/// nothing.
pub async fn ingest_receipt<B: OfflineBooks + ?Sized>(
    conn: &mut PgConnection,
    upload: &Upload<'_>,
    accounts: OfflineAccounts,
    books: &B,
    options: &PostOptions,
) -> Result<IngestResult, OfflineError> {
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
    let ctx = Ctx {
        books,
        accounts,
        now: upload.now,
        options,
    };
    let mut attempt = 1;
    loop {
        let result = async {
            let mut tx = options.begin(conn).await?;
            match ingest_in(&mut tx, &receipt, upload, &ctx).await {
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

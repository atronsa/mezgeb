//! Offline receipt ingestion on PostgreSQL (step 2.12): exactly once,
//! whatever the order, the uploader or the repeats; refusals write nothing;
//! conflicts and over-spends freeze; and the escrow always holds what the
//! voucher has left.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use mezgeb_core::{
    Account, AccountId, Chart, ClientId, Currency, DeviceId, EntryDraft, IdempotencyKey, Ledger,
    Money, PositiveAmount, Timestamp, VoucherId,
};
use mezgeb_engine::{
    Attestation, AttestationKind, DeviceStatus, FeePolicy, IngestResult, IssuedVoucher, IssuerKey,
    NewDevice, NewVoucher, PgStore, PostOptions, SettleRequest, Upload, create_client,
    escrow_findings, get_device, get_voucher, ingest_receipt, issue_offline_voucher,
    open_account_keyed, register_device, set_offline_fee, voucher_receipts,
};
use mezgeb_offline::{Outcome, Receipt, SecretKey, Tier};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use support::{Server, TestDb, TestResult, uuid7};

const CHART: &str = include_str!("../../../charts/wallet-provider.toml");
const BANK: u64 = 1;
const HANA: u64 = 2;
const COFFEE: u64 = 3;
const STALL: u64 = 4;
const FEES: u64 = 5;
const OTHER_MERCHANT: u64 = 6;
/// 2026-11-16T00:00:00Z.
const NOW: i64 = 1_794_787_200_000_000;
const HOUR: i64 = 3_600_000_000;

type BoxError = Box<dyn std::error::Error>;
/// A receipt's body and its signature, as uploaded.
type Signed = (Vec<u8>, Vec<u8>);

fn id<T: std::str::FromStr>(n: u64) -> Result<T, BoxError>
where
    T::Err: std::fmt::Display,
{
    uuid7(n).parse().map_err(|e: T::Err| e.to_string().into())
}

fn etb(minor: i64) -> Money {
    Money::from_minor(minor, Currency::ETB)
}

fn key(label: &str) -> Result<SecretKey, BoxError> {
    Ok(SecretKey::from_bytes(
        &Sha256::digest(label.as_bytes()).into(),
    )?)
}

fn issue_book(
    voucher: VoucherId,
    amount: PositiveAmount,
) -> impl Fn(&BTreeMap<AccountId, Account>, AccountId, AccountId) -> Result<EntryDraft, String> {
    move |_, wallet, escrow| {
        let mut draft = EntryDraft::default()
            .debit(wallet, amount.money())
            .credit(escrow, amount.money());
        draft.metadata.insert("voucher".into(), voucher.to_string());
        Ok(draft)
    }
}

/// Books E17 as `offline_receipt_settle.v1` does.
fn settle_book(_: &BTreeMap<AccountId, Account>, r: &SettleRequest) -> Result<EntryDraft, String> {
    let net = Money::from_minor(r.amount.amount() - r.fee.amount(), r.amount.currency());
    let mut draft = EntryDraft::default()
        .debit(r.escrow, r.amount)
        .credit(r.payee, net);
    if let Some(fees) = r.fee_account {
        draft = draft.credit(fees, r.fee);
    }
    draft.event_time = r.event_time;
    for (k, v) in [
        ("voucher", r.voucher.to_string()),
        ("device", r.device.to_string()),
        ("counter", r.counter.to_string()),
    ] {
        draft.metadata.insert(k.into(), v);
    }
    Ok(draft)
}

/// A ledger with one client, Hana's funded wallet, two merchants, fee
/// revenue, Hana's phone and its voucher.
struct World {
    db: TestDb,
    client: ClientId,
    device: DeviceId,
    device_key: SecretKey,
    voucher: IssuedVoucher,
}

async fn world(voucher_minor: i64, fee_bps: u32) -> Result<Option<World>, BoxError> {
    let Some(server) = Server::from_env().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let db = server.migrated().await.map_err(|e| e.to_string())?;
    let client: ClientId = id(0xc11e)?;
    let mut admin = db.admin().await?;
    create_client(&mut admin, client, "acme", 1_000, 1_000).await?;
    set_offline_fee(&mut admin, client, fee_bps).await?;
    // Another client, with its own merchant.
    let other: ClientId = id(0xc22e)?;
    create_client(&mut admin, other, "other", 1_000, 1_000).await?;
    drop(admin);
    let chart = Chart::from_toml(CHART)?;
    let mut app = db.app().await?;
    for (owner, n, reference) in [
        (client, BANK, "1010"),
        (client, HANA, "2010:cust-hana"),
        (client, COFFEE, "2030:merch-coffee"),
        (client, STALL, "2030:merch-stall"),
        (client, FEES, "4010"),
        (other, OTHER_MERCHANT, "2030:merch-other"),
    ] {
        let key = IdempotencyKey::new(format!("open-{reference}"))?;
        open_account_keyed(&mut app, owner, &key, id(n)?, &chart, reference).await?;
    }
    drop(app);
    let mut store = PgStore::new(db.app_pool().await?, client);
    store
        .post(
            id(9_000)?,
            EntryDraft::default()
                .debit(id(BANK)?, etb(1_000_000))
                .credit(id(HANA)?, etb(1_000_000)),
        )
        .await?;

    let issuer = IssuerKey::from_secret(key("issuer")?);
    let device_key = key("hana's phone")?;
    let mut conn = db.app().await?;
    let device = register_device(
        &mut conn,
        &issuer,
        NewDevice {
            id: id(100)?,
            client,
            owner_account: id(HANA)?,
            public_key: device_key.public_key().clone(),
            attestation: Attestation {
                kind: AttestationKind::Card,
                tier: Tier::A,
                detail: "test".into(),
            },
            label: "Hana's card".into(),
            now: Timestamp::from_micros(NOW),
        },
    )
    .await?
    .device()
    .id();
    let amount = PositiveAmount::new(etb(voucher_minor))?;
    let new = NewVoucher {
        voucher_id: id(200)?,
        device_id: device,
        client,
        entry_id: id(300)?,
        escrow_account: id(400)?,
        idempotency_key: IdempotencyKey::new("voucher-1")?,
        amount,
        now: Timestamp::from_micros(NOW),
    };
    let voucher = issue_offline_voucher(
        &mut conn,
        &issuer,
        &chart,
        &new,
        issue_book(new.voucher_id, amount),
        &PostOptions::default(),
    )
    .await?;
    Ok(Some(World {
        db,
        client,
        device,
        device_key,
        voucher,
    }))
}

impl World {
    /// A chain of receipts of the voucher, to the given payees and amounts.
    fn chain(&self, payments: &[(u64, i64)]) -> Result<Vec<Signed>, BoxError> {
        let mut prev = self.voucher.voucher.digest();
        let mut out = Vec::new();
        for (i, (payee, minor)) in payments.iter().enumerate() {
            let n = i as u64;
            let receipt = Receipt {
                device_id: self.device,
                voucher_id: self.voucher.voucher.voucher_id,
                counter: self.voucher.voucher.counter_start + n,
                prev_digest: prev,
                payee_account_id: id(*payee)?,
                payee_device_id: None,
                amount: etb(*minor),
                event_time: Timestamp::from_micros(NOW + (n as i64 + 1) * HOUR),
                payee_challenge: [n as u8; 16],
            };
            prev = receipt.digest();
            out.push((receipt.body(), receipt.sign(&self.device_key).to_vec()));
        }
        Ok(out)
    }

    fn signed(&self, receipt: &Receipt) -> Signed {
        (receipt.body(), receipt.sign(&self.device_key).to_vec())
    }

    async fn upload(
        &self,
        conn: &mut PgConnection,
        (body, signature): &Signed,
    ) -> Result<IngestResult, BoxError> {
        let upload = Upload {
            client: self.client,
            body,
            signature,
            now: Timestamp::from_micros(NOW + 24 * HOUR),
        };
        let policy = FeePolicy {
            fee_account: Some(id(FEES)?),
        };
        Ok(ingest_receipt(conn, &upload, policy, settle_book, &PostOptions::default()).await?)
    }
}

async fn balance(conn: &mut PgConnection, account: AccountId) -> Result<i64, BoxError> {
    Ok(mezgeb_engine::get_account(conn, account)
        .await?
        .ok_or("no account")?
        .balance()
        .amount())
}

async fn count(conn: &mut PgConnection, sql: &str) -> Result<i64, BoxError> {
    Ok(
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(conn)
            .await?,
    )
}

/// The final state that must not depend on the upload order: each
/// receipt's outcome and entry presence, and the balances.
async fn state(w: &World, conn: &mut PgConnection) -> Result<String, BoxError> {
    let receipts = voucher_receipts(conn, w.client, w.voucher.voucher.voucher_id).await?;
    let mut out = String::new();
    for r in receipts {
        out += &format!(
            "{}:{}:{} ",
            r.counter,
            r.outcome.as_str(),
            r.entry.is_some()
        );
    }
    for n in [HANA, COFFEE, STALL, FEES] {
        out += &format!("{}={} ", n, balance(conn, id(n)?).await?);
    }
    out += &format!(
        "escrow={}",
        balance(conn, w.voucher.voucher.escrow_account_id).await?
    );
    Ok(out)
}

#[tokio::test]
async fn payer_and_payee_upload_the_same_receipt_once() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let receipts = w.chain(&[(COFFEE, 6_000)])?;

    let first = w.upload(&mut conn, &receipts[0]).await?;
    assert_eq!(first.outcome, Outcome::Settled);
    assert_eq!(first.settled.len(), 1);
    let second = w.upload(&mut conn, &receipts[0]).await?;
    assert_eq!(second.outcome, Outcome::Duplicate);
    assert_eq!(second.entry, first.entry);

    // E17: 60 ETB from escrow, 58.80 to the merchant, 1.20 in fees.
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        4_000
    );
    assert_eq!(balance(&mut conn, id(COFFEE)?).await?, 5_880);
    assert_eq!(balance(&mut conn, id(FEES)?).await?, 120);
    let entry = mezgeb_engine::entry_record(&mut conn, first.entry.ok_or("no entry")?)
        .await?
        .ok_or("no record")?;
    let key = IdempotencyKey::new(format!("offline:{}:1", w.device))?;
    let by_key = mezgeb_engine::entry_record_by_key(&mut conn, w.client, &key)
        .await?
        .ok_or("no entry under the key")?;
    assert_eq!(Some(by_key.entry.id()), first.entry);
    assert_eq!(
        entry.entry.event_time(),
        Some(Timestamp::from_micros(NOW + HOUR))
    );
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop(conn);
    w.db.drop().await?;
    Ok(())
}

/// Reversed order: each receipt waits for its predecessor, and the first
/// settles them all, in order. The same final state as in order.
#[tokio::test]
async fn any_upload_order_gives_the_same_final_state() -> TestResult {
    let payments = [
        (COFFEE, 2_000),
        (STALL, 1_500),
        (COFFEE, 999),
        (STALL, 3_001),
    ];
    let mut states = Vec::new();
    for order in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1]] {
        let Some(w) = world(10_000, 200).await? else {
            return Ok(());
        };
        let mut conn = w.db.app().await?;
        let receipts = w.chain(&payments)?;
        for (step, i) in order.iter().enumerate() {
            let result = w.upload(&mut conn, &receipts[*i]).await?;
            if order == [3, 2, 1, 0] && step < 3 {
                assert_eq!(result.outcome, Outcome::WaitingForGap);
            }
        }
        if order == [3, 2, 1, 0] {
            // The last upload, counter 1, settled all four.
            let all = voucher_receipts(&mut conn, w.client, w.voucher.voucher.voucher_id).await?;
            assert!(all.iter().all(|r| r.outcome == Outcome::Settled));
        }
        states.push(state(&w, &mut conn).await?);
        assert_eq!(escrow_findings(&mut conn).await?, vec![]);
        drop(conn);
        w.db.drop().await?;
    }
    assert!(states.windows(2).all(|s| s[0] == s[1]), "{states:#?}");
    Ok(())
}

#[tokio::test]
async fn a_forged_receipt_changes_nothing() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let tables = "SELECT (SELECT count(*) FROM offline_receipts)
                       + (SELECT count(*) FROM journal_entries)
                       + (SELECT count(*) FROM audit_events)";
    let before = count(&mut conn, tables).await?;

    let receipts = w.chain(&[(COFFEE, 6_000)])?;
    // Signed by another key.
    let mut receipt = Receipt::decode(&receipts[0].0)?;
    let forged = (receipt.body(), receipt.sign(&key("thief")?).to_vec());
    assert_eq!(
        w.upload(&mut conn, &forged).await?.outcome,
        Outcome::BadSignature
    );
    // The amount changed after signing.
    receipt.amount = etb(9_999);
    let altered = (receipt.body(), receipts[0].1.clone());
    assert_eq!(
        w.upload(&mut conn, &altered).await?.outcome,
        Outcome::BadSignature
    );
    // Not a receipt at all.
    let junk = (b"hello".to_vec(), receipts[0].1.clone());
    assert_eq!(
        w.upload(&mut conn, &junk).await?.outcome,
        Outcome::BadFormat
    );

    assert_eq!(count(&mut conn, tables).await?, before);
    assert_eq!(
        get_device(&mut conn, w.client, w.device).await?.status,
        DeviceStatus::Active
    );
    // The genuine one still settles.
    assert_eq!(
        w.upload(&mut conn, &receipts[0]).await?.outcome,
        Outcome::Settled
    );
    drop(conn);
    w.db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn refusals_name_what_is_wrong() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let base = Receipt::decode(&w.chain(&[(COFFEE, 100)])?[0].0)?;

    let other_device = Receipt {
        device_id: id(999)?,
        ..base.clone()
    };
    assert_eq!(
        w.upload(&mut conn, &w.signed(&other_device)).await?.outcome,
        Outcome::UnknownDevice
    );
    let other_voucher = Receipt {
        voucher_id: id(998)?,
        ..base.clone()
    };
    assert_eq!(
        w.upload(&mut conn, &w.signed(&other_voucher))
            .await?
            .outcome,
        Outcome::UnknownVoucher
    );
    let before_start = Receipt {
        counter: 0,
        ..base.clone()
    };
    assert_eq!(
        w.upload(&mut conn, &w.signed(&before_start)).await?.outcome,
        Outcome::UnknownVoucher
    );
    // A payee of another client, or not a wallet or merchant.
    for payee in [OTHER_MERCHANT, BANK] {
        let wrong = Receipt {
            payee_account_id: id(payee)?,
            ..base.clone()
        };
        assert_eq!(
            w.upload(&mut conn, &w.signed(&wrong)).await?.outcome,
            Outcome::BadFormat
        );
    }
    // None of them was recorded.
    assert_eq!(
        count(&mut conn, "SELECT count(*) FROM offline_receipts").await?,
        0
    );
    drop(conn);
    w.db.drop().await?;
    Ok(())
}

/// More than the escrow left: OVER_LIMIT, the device and voucher freeze,
/// what settled stays settled, and later receipts are kept, not paid.
#[tokio::test]
async fn overspending_the_voucher_freezes_the_device() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let receipts = w.chain(&[(COFFEE, 7_000), (STALL, 5_000), (COFFEE, 100)])?;
    assert_eq!(
        w.upload(&mut conn, &receipts[0]).await?.outcome,
        Outcome::Settled
    );
    assert_eq!(
        w.upload(&mut conn, &receipts[1]).await?.outcome,
        Outcome::OverLimit
    );
    assert_eq!(
        w.upload(&mut conn, &receipts[2]).await?.outcome,
        Outcome::Frozen
    );

    assert_eq!(
        get_device(&mut conn, w.client, w.device).await?.status,
        DeviceStatus::Frozen
    );
    let voucher = get_voucher(&mut conn, w.client, w.voucher.voucher.voucher_id).await?;
    assert_eq!(voucher.status.as_str(), "frozen");
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        3_000
    );
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 0);
    // The alert.
    assert_eq!(
        count(
            &mut conn,
            "SELECT count(*) FROM audit_events WHERE action = 'offline.freeze'"
        )
        .await?,
        1
    );
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop(conn);
    w.db.drop().await?;
    Ok(())
}

/// A cloned device: two receipts with one counter. The first stands; the
/// second is kept as evidence, and the device freezes (INV-21).
#[tokio::test]
async fn two_receipts_with_one_counter_are_a_conflict() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let to_coffee = w.chain(&[(COFFEE, 5_000)])?;
    let to_stall = w.chain(&[(STALL, 6_000)])?;
    assert_eq!(
        w.upload(&mut conn, &to_coffee[0]).await?.outcome,
        Outcome::Settled
    );
    let clone = w.upload(&mut conn, &to_stall[0]).await?;
    assert_eq!(clone.outcome, Outcome::Conflict);
    assert_eq!(clone.entry, None);
    // Uploading the clone again is still a conflict, kept once.
    assert_eq!(
        w.upload(&mut conn, &to_stall[0]).await?.outcome,
        Outcome::Conflict
    );
    assert_eq!(
        count(&mut conn, "SELECT count(*) FROM offline_receipt_conflicts").await?,
        1
    );
    assert_eq!(
        get_device(&mut conn, w.client, w.device).await?.status,
        DeviceStatus::Frozen
    );
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 0);
    assert_eq!(balance(&mut conn, id(COFFEE)?).await?, 5_000);

    // A receipt whose link does not match its predecessor: a conflict too.
    let Some(w2) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn2 = w2.db.app().await?;
    let chain = w2.chain(&[(COFFEE, 100), (COFFEE, 200)])?;
    let mut bad_link = Receipt::decode(&chain[1].0)?;
    bad_link.prev_digest = [9; 32];
    assert_eq!(
        w2.upload(&mut conn2, &chain[0]).await?.outcome,
        Outcome::Settled
    );
    assert_eq!(
        w2.upload(&mut conn2, &w2.signed(&bad_link)).await?.outcome,
        Outcome::Conflict
    );
    assert_eq!(
        get_device(&mut conn2, w2.client, w2.device).await?.status,
        DeviceStatus::Frozen
    );
    drop((conn, conn2));
    w.db.drop().await?;
    w2.db.drop().await?;
    Ok(())
}

/// The same receipt uploaded 100 times at once settles once.
#[tokio::test]
async fn a_hundred_concurrent_uploads_settle_once() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let w = Arc::new(w);
    let pool = w.db.app_pool_of(20).await?;
    let receipt = Arc::new(w.chain(&[(COFFEE, 6_000)])?.remove(0));
    let mut tasks = Vec::new();
    for _ in 0..100 {
        let (w, pool, receipt) = (w.clone(), pool.clone(), receipt.clone());
        tasks.push(tokio::spawn(async move {
            let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
            w.upload(&mut conn, &receipt)
                .await
                .map(|r| r.outcome)
                .map_err(|e| e.to_string())
        }));
    }
    let mut outcomes = BTreeMap::new();
    for task in tasks {
        *outcomes.entry(task.await??.as_str()).or_insert(0) += 1;
    }
    assert_eq!(outcomes.get("SETTLED"), Some(&1), "{outcomes:?}");
    assert_eq!(outcomes.get("DUPLICATE"), Some(&99), "{outcomes:?}");
    let mut conn = pool.acquire().await?;
    assert_eq!(balance(&mut conn, id(COFFEE)?).await?, 5_880);
    assert_eq!(
        count(
            &mut conn,
            "SELECT count(*) FROM journal_entries WHERE idempotency_key LIKE 'offline:%'"
        )
        .await?,
        1
    );
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop((conn, pool));
    Arc::try_unwrap(w)
        .map_err(|_| "world still shared")?
        .db
        .drop()
        .await?;
    Ok(())
}

/// A settled receipt can never be changed, even by the service's role.
#[tokio::test]
async fn a_settled_receipt_never_changes() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    w.upload(&mut conn, &w.chain(&[(COFFEE, 100)])?[0]).await?;
    let changed = sqlx::query("UPDATE offline_receipts SET outcome = 'waiting_for_gap'")
        .execute(&mut conn)
        .await;
    let code = changed.err().and_then(|e| {
        e.as_database_error()
            .and_then(|d| d.code().map(|c| c.to_string()))
    });
    assert_eq!(code.as_deref(), Some("MZ011"));
    drop(conn);
    w.db.drop().await?;
    Ok(())
}

/// The acceptance criterion: random upload orders, with repeats, always
/// give the same final state; each receipt settles once (INV-20) and the
/// escrow never pays more than the voucher (INV-19).
#[tokio::test]
async fn random_upload_orders_settle_each_receipt_once() -> TestResult {
    let payments = [
        (COFFEE, 1_200),
        (STALL, 800),
        (COFFEE, 2_501),
        (STALL, 999),
        (COFFEE, 3_000),
    ];
    let mut expected = None;
    // A small deterministic generator: the same orders on every run.
    let mut seed: u64 = 0x005e_ed0f_0ff1;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for _ in 0..8 {
        let Some(w) = world(10_000, 200).await? else {
            return Ok(());
        };
        let mut conn = w.db.app().await?;
        let receipts = w.chain(&payments)?;
        // Every receipt once, in a random order, then some repeats.
        let mut order: Vec<usize> = (0..receipts.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, next(i + 1));
        }
        for _ in 0..4 {
            order.insert(next(order.len() + 1), next(receipts.len()));
        }
        let mut settles = 0;
        for i in &order {
            settles += w.upload(&mut conn, &receipts[*i]).await?.settled.len();
        }
        assert_eq!(settles, receipts.len(), "order {order:?}");
        let state = state(&w, &mut conn).await?;
        match &expected {
            None => expected = Some(state),
            Some(first) => assert_eq!(&state, first, "order {order:?}"),
        }
        assert_eq!(escrow_findings(&mut conn).await?, vec![]);
        drop(conn);
        w.db.drop().await?;
    }
    Ok(())
}

//! The offline test world shared by the receipt, lifecycle and scenario
//! tests: a client, Hana's funded wallet, two merchants, the fee and loss
//! accounts, Hana's device and its voucher, and the books.

#![allow(dead_code)]

use std::collections::BTreeMap;

use mezgeb_core::{
    Account, AccountId, Chart, ClientId, Currency, DeviceId, EntryDraft, IdempotencyKey, Ledger,
    Money, PositiveAmount, Timestamp, VoucherId,
};
use mezgeb_engine::{
    Attestation, AttestationKind, IngestResult, IssuedVoucher, IssuerKey, NewDevice, NewVoucher,
    OfflineAccounts, OfflineBooks, PayRequest, PgStore, PostOptions, RepayRequest, ReturnRequest,
    SettleRequest, TickReport, Upload, create_client, ingest_receipt, issue_offline_voucher,
    offline_tick, open_account_keyed, register_device, set_offline_fee, voucher_receipts,
};
use mezgeb_offline::{Receipt, SecretKey, Tier};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

use crate::support::{Server, TestDb, uuid7};

pub const CHART: &str = include_str!("../../../../charts/wallet-provider.toml");
pub const BANK: u64 = 1;
pub const HANA: u64 = 2;
pub const COFFEE: u64 = 3;
pub const STALL: u64 = 4;
pub const FEES: u64 = 5;
pub const OTHER_MERCHANT: u64 = 6;
pub const LOSS: u64 = 7;
/// 2026-11-16T00:00:00Z.
pub const NOW: i64 = 1_794_787_200_000_000;
pub const HOUR: i64 = 3_600_000_000;

pub type BoxError = Box<dyn std::error::Error>;
/// A receipt's body and its signature, as uploaded.
pub type Signed = (Vec<u8>, Vec<u8>);

pub fn id<T: std::str::FromStr>(n: u64) -> Result<T, BoxError>
where
    T::Err: std::fmt::Display,
{
    uuid7(n).parse().map_err(|e: T::Err| e.to_string().into())
}

pub fn etb(minor: i64) -> Money {
    Money::from_minor(minor, Currency::ETB)
}

pub fn key(label: &str) -> Result<SecretKey, BoxError> {
    Ok(SecretKey::from_bytes(
        &Sha256::digest(label.as_bytes()).into(),
    )?)
}

pub fn issue_book(
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

/// Books the offline entries as the templates do.
pub struct TestBooks;

fn named(mut draft: EntryDraft, pairs: &[(&str, String)]) -> EntryDraft {
    for (k, v) in pairs {
        draft.metadata.insert((*k).into(), v.clone());
    }
    draft
}

impl OfflineBooks for TestBooks {
    fn settle(
        &self,
        _: &BTreeMap<AccountId, Account>,
        r: &SettleRequest,
    ) -> Result<EntryDraft, String> {
        let net = Money::from_minor(r.amount.amount() - r.fee.amount(), r.amount.currency());
        let mut draft = EntryDraft::default()
            .debit(r.escrow, r.amount)
            .credit(r.payee, net);
        if let Some(fees) = r.fee_account {
            draft = draft.credit(fees, r.fee);
        }
        draft.event_time = r.event_time;
        Ok(named(
            draft,
            &[
                ("voucher", r.voucher.to_string()),
                ("device", r.device.to_string()),
                ("counter", r.counter.to_string()),
            ],
        ))
    }

    fn pay(&self, _: &BTreeMap<AccountId, Account>, r: &PayRequest) -> Result<EntryDraft, String> {
        let mut draft = EntryDraft::default();
        let mut total = 0;
        for (account, part) in [
            (r.escrow, r.from_escrow),
            (r.wallet, r.from_wallet),
            (r.loss_account, r.from_loss),
        ] {
            if let Some(account) = account {
                draft = draft.debit(account, part);
                total += part.amount();
            }
        }
        let currency = r.fee.currency();
        draft = draft.credit(r.payee, Money::from_minor(total - r.fee.amount(), currency));
        if let Some(fees) = r.fee_account {
            draft = draft.credit(fees, r.fee);
        }
        draft.event_time = r.event_time;
        Ok(named(
            draft,
            &[
                ("voucher", r.voucher.to_string()),
                ("device", r.device.to_string()),
                ("counter", r.counter.to_string()),
            ],
        ))
    }

    fn return_escrow(
        &self,
        _: &BTreeMap<AccountId, Account>,
        r: &ReturnRequest,
    ) -> Result<EntryDraft, String> {
        let draft = EntryDraft::default()
            .debit(r.escrow, r.amount)
            .credit(r.wallet, r.amount);
        Ok(named(
            draft,
            &[
                ("voucher", r.voucher.to_string()),
                ("reason", r.reason.to_owned()),
            ],
        ))
    }

    fn repay(
        &self,
        _: &BTreeMap<AccountId, Account>,
        r: &RepayRequest,
    ) -> Result<EntryDraft, String> {
        let draft = EntryDraft::default()
            .debit(r.wallet, r.amount)
            .credit(r.loss_account, r.amount);
        Ok(named(draft, &[("shortfall", r.shortfall.to_string())]))
    }
}

/// When [`World::upload`] uploads, in ledger time: a receipt's
/// `received_at`, from which its gap timeout runs.
pub const UPLOADED: i64 = NOW + 24 * HOUR;

pub const DAY: i64 = 24 * HOUR;

pub fn accounts() -> OfflineAccounts {
    OfflineAccounts {
        fee_account: uuid7(FEES).parse().ok(),
        loss_account: uuid7(LOSS).parse().ok(),
    }
}

/// A ledger with one client, Hana's funded wallet, two merchants, fee
/// revenue, Hana's phone and its voucher.
pub struct World {
    pub db: TestDb,
    pub client: ClientId,
    pub device: DeviceId,
    pub device_key: SecretKey,
    pub voucher: IssuedVoucher,
}

pub async fn world(voucher_minor: i64, fee_bps: u32) -> Result<Option<World>, BoxError> {
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
        (client, LOSS, "5030"),
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
    pub fn chain(&self, payments: &[(u64, i64)]) -> Result<Vec<Signed>, BoxError> {
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

    pub fn signed(&self, receipt: &Receipt) -> Signed {
        (receipt.body(), receipt.sign(&self.device_key).to_vec())
    }

    pub async fn upload(
        &self,
        conn: &mut PgConnection,
        (body, signature): &Signed,
    ) -> Result<IngestResult, BoxError> {
        let upload = Upload {
            client: self.client,
            body,
            signature,
            now: Timestamp::from_micros(UPLOADED),
        };
        Ok(ingest_receipt(
            conn,
            &upload,
            accounts(),
            &TestBooks,
            &PostOptions::default(),
        )
        .await?)
    }

    /// Uploads at a given ledger time.
    pub async fn upload_at(
        &self,
        conn: &mut PgConnection,
        (body, signature): &Signed,
        now: i64,
    ) -> Result<IngestResult, BoxError> {
        let upload = Upload {
            client: self.client,
            body,
            signature,
            now: Timestamp::from_micros(now),
        };
        Ok(ingest_receipt(
            conn,
            &upload,
            accounts(),
            &TestBooks,
            &PostOptions::default(),
        )
        .await?)
    }

    /// Issues another voucher to Hana's device, at a given time.
    pub async fn issue_again(
        &self,
        conn: &mut PgConnection,
        n: u64,
        minor: i64,
        now: i64,
    ) -> Result<Result<IssuedVoucher, mezgeb_engine::OfflineError>, BoxError> {
        let amount = PositiveAmount::new(etb(minor))?;
        let new = NewVoucher {
            voucher_id: id(200 + n)?,
            device_id: self.device,
            client: self.client,
            entry_id: id(300 + n)?,
            escrow_account: id(400 + n)?,
            idempotency_key: IdempotencyKey::new(format!("voucher-{n}"))?,
            amount,
            now: Timestamp::from_micros(now),
        };
        let issuer = IssuerKey::from_secret(key("issuer")?);
        let chart = Chart::from_toml(CHART)?;
        Ok(issue_offline_voucher(
            conn,
            &issuer,
            &chart,
            &new,
            issue_book(new.voucher_id, amount),
            &PostOptions::default(),
        )
        .await)
    }

    /// Runs the offline tick at a given ledger time.
    pub async fn tick(&self, conn: &mut PgConnection, now: i64) -> Result<TickReport, BoxError> {
        let accounts = accounts();
        Ok(offline_tick(
            conn,
            Timestamp::from_micros(now),
            accounts,
            &TestBooks,
            &PostOptions::default(),
        )
        .await?)
    }

    /// Hana spends from her wallet online, to the bank, leaving `left`.
    pub async fn spend_wallet_to(&self, left: i64) -> Result<(), BoxError> {
        let mut conn = self.db.app().await?;
        let balance = balance(&mut conn, id(HANA)?).await?;
        drop(conn);
        if balance > left {
            let mut store = PgStore::new(self.db.app_pool().await?, self.client);
            let entry: mezgeb_core::EntryId = uuid7(80_000 + balance as u64).parse()?;
            store
                .post(
                    entry,
                    EntryDraft::default()
                        .debit(id(HANA)?, etb(balance - left))
                        .credit(id(BANK)?, etb(balance - left)),
                )
                .await?;
        }
        Ok(())
    }
}

pub async fn balance(conn: &mut PgConnection, account: AccountId) -> Result<i64, BoxError> {
    Ok(mezgeb_engine::get_account(conn, account)
        .await?
        .ok_or("no account")?
        .balance()
        .amount())
}

pub async fn count(conn: &mut PgConnection, sql: &str) -> Result<i64, BoxError> {
    Ok(
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(conn)
            .await?,
    )
}

/// The final state that must not depend on the upload order: each
/// receipt's outcome and entry presence, and the balances.
pub async fn state(w: &World, conn: &mut PgConnection) -> Result<String, BoxError> {
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

//! Offline devices and vouchers on PostgreSQL (step 2.11): registration,
//! issue against the tier limits and the customer cap, frozen and revoked
//! devices, signatures, concurrency, and the escrow invariant.

mod support;

use std::collections::BTreeMap;

use mezgeb_core::{
    Account, AccountId, Chart, ClientId, Currency, DeviceId, EntryDraft, EntryId, IdempotencyKey,
    Ledger, LedgerError, Money, PositiveAmount, PostingError, Timestamp, VoucherId,
};
use mezgeb_engine::{
    Attestation, AttestationKind, DeviceStatus, IssuedVoucher, IssuerKey, NewDevice, NewVoucher,
    OfflineError, PgStore, PostOptions, Registered, VoucherStatus, create_client, escrow_findings,
    issue_offline_voucher, register_device, set_device_status, set_tier_limits,
};
use mezgeb_offline::{PublicKey, SecretKey, Tier, TierLimits};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use support::{Server, TestDb, TestResult, uuid7};

const CHART: &str = include_str!("../../../charts/wallet-provider.toml");
const BANK: u64 = 1;
const HANA: u64 = 2;
const COFFEE: u64 = 3;
/// 2026-11-16T00:00:00Z.
const NOW: i64 = 1_794_787_200_000_000;

fn id<T: std::str::FromStr>(n: u64) -> Result<T, Box<dyn std::error::Error>>
where
    T::Err: std::fmt::Display,
{
    uuid7(n).parse().map_err(|e: T::Err| e.to_string().into())
}

fn etb(minor: i64) -> Money {
    Money::from_minor(minor, Currency::ETB)
}

fn positive(minor: i64) -> Result<PositiveAmount, mezgeb_core::MoneyError> {
    PositiveAmount::new(etb(minor))
}

fn key(label: &str) -> Result<SecretKey, Box<dyn std::error::Error>> {
    Ok(SecretKey::from_bytes(
        &Sha256::digest(label.as_bytes()).into(),
    )?)
}

/// Books E16 as `issue_offline_voucher.v1` does.
fn book(
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

struct Setup {
    db: TestDb,
    client: ClientId,
    issuer: IssuerKey,
    chart: Chart,
}

/// A client, the bank, Hana's wallet holding `funds` minor units, and
/// Merkato Coffee's balance.
async fn setup(funds: i64) -> Result<Option<Setup>, Box<dyn std::error::Error>> {
    let Some(server) = Server::from_env()? else {
        return Ok(None);
    };
    let db = server.migrated().await?;
    let client: ClientId = id(0xc11e)?;
    create_client(&mut db.admin().await?, client, "acme", 1_000, 1_000).await?;
    let chart = Chart::from_toml(CHART)?;
    let mut store = PgStore::new(db.app_pool().await?, client);
    for (n, reference) in [
        (BANK, "1010"),
        (HANA, "2010:cust-hana"),
        (COFFEE, "2030:merch-coffee"),
    ] {
        store.open_account(id(n)?, &chart, reference).await?;
    }
    store
        .post(
            id(9_000)?,
            EntryDraft::default()
                .debit(id(BANK)?, etb(funds))
                .credit(id(HANA)?, etb(funds)),
        )
        .await?;
    Ok(Some(Setup {
        db,
        client,
        issuer: IssuerKey::from_secret(key("issuer")?),
        chart,
    }))
}

impl Setup {
    async fn register(
        &self,
        conn: &mut PgConnection,
        n: u64,
        tier: Tier,
    ) -> Result<DeviceId, Box<dyn std::error::Error>> {
        let kind = match tier {
            Tier::A => AttestationKind::Card,
            Tier::B => AttestationKind::Android,
            Tier::C => AttestationKind::None,
        };
        let device = register_device(
            conn,
            &self.issuer,
            NewDevice {
                id: id(100 + n)?,
                client: self.client,
                owner_account: id(HANA)?,
                public_key: key(&format!("device {n}"))?.public_key().clone(),
                attestation: Attestation {
                    kind,
                    tier,
                    detail: "test".into(),
                },
                label: format!("device {n}"),
                now: Timestamp::from_micros(NOW),
            },
        )
        .await?;
        Ok(device.device().id())
    }

    fn voucher(
        &self,
        device: DeviceId,
        n: u64,
        minor: i64,
    ) -> Result<NewVoucher, Box<dyn std::error::Error>> {
        Ok(NewVoucher {
            voucher_id: id(200 + n)?,
            device_id: device,
            client: self.client,
            entry_id: id(300 + n)?,
            escrow_account: id(400 + n)?,
            idempotency_key: IdempotencyKey::new(format!("voucher-{n}"))?,
            amount: positive(minor)?,
            now: Timestamp::from_micros(NOW),
        })
    }

    async fn issue(
        &self,
        conn: &mut PgConnection,
        device: DeviceId,
        n: u64,
        minor: i64,
    ) -> Result<Result<IssuedVoucher, OfflineError>, Box<dyn std::error::Error>> {
        let new = self.voucher(device, n, minor)?;
        let booked = book(new.voucher_id, new.amount);
        Ok(issue_offline_voucher(
            conn,
            &self.issuer,
            &self.chart,
            &new,
            booked,
            &PostOptions::default(),
        )
        .await)
    }
}

async fn balance(conn: &mut PgConnection, n: u64) -> Result<i64, Box<dyn std::error::Error>> {
    let account = mezgeb_engine::get_account(conn, id(n)?)
        .await?
        .ok_or("no account")?;
    Ok(account.balance().amount())
}

/// E16: the wallet funds a new escrow, and the voucher is signed by the
/// issuer key; one flipped bit breaks the signature.
#[tokio::test]
async fn a_voucher_reserves_escrow_and_is_signed() -> TestResult {
    let Some(s) = setup(1_000_000).await? else {
        return Ok(());
    };
    let mut conn = s.db.app().await?;
    let device = s.register(&mut conn, 1, Tier::B).await?;
    let issued = s.issue(&mut conn, device, 1, 100_000).await??;

    assert_eq!(issued.status, VoucherStatus::Active);
    assert_eq!(issued.voucher.counter_start, 1);
    assert_eq!(issued.voucher.amount, etb(100_000));
    assert_eq!(
        issued.voucher.expires_at.as_micros() - NOW,
        Tier::B.maximum().lifetime_micros
    );
    assert_eq!(balance(&mut conn, HANA).await?, 900_000);
    let escrow = mezgeb_engine::get_account(&mut conn, issued.voucher.escrow_account_id)
        .await?
        .ok_or("no escrow")?;
    assert_eq!(
        escrow.reference(),
        format!("2050:vch-{}", issued.voucher.voucher_id)
    );
    assert_eq!(escrow.balance(), etb(100_000));

    let issuer = PublicKey::from_compressed(s.issuer.public_key().compressed())?;
    assert_eq!(issued.voucher.verify(&issuer, &issued.signature), Ok(()));
    let body = issued.voucher.body();
    for bit in [0, 8 * body.len() / 2, 8 * body.len() - 1] {
        let mut tampered = body.clone();
        tampered[bit / 8] ^= 1 << (bit % 8);
        assert!(issuer.verify(&tampered, &issued.signature).is_err());
    }
    let mut changed = issued.voucher.clone();
    changed.amount = etb(100_001);
    assert!(changed.verify(&issuer, &issued.signature).is_err());

    // The same request again: the same voucher, nothing new posted.
    let mut retry = s.voucher(device, 1, 100_000)?;
    retry.voucher_id = id(299)?;
    retry.entry_id = id(399)?;
    retry.escrow_account = id(499)?;
    let booked = book(retry.voucher_id, retry.amount);
    let again = issue_offline_voucher(
        &mut conn,
        &s.issuer,
        &s.chart,
        &retry,
        booked,
        &PostOptions::default(),
    )
    .await?;
    assert_eq!(again, issued);
    assert_eq!(balance(&mut conn, HANA).await?, 900_000);

    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop(conn);
    s.db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn issuing_beyond_the_tier_limit_is_refused() -> TestResult {
    let Some(s) = setup(1_000_000).await? else {
        return Ok(());
    };
    let mut conn = s.db.app().await?;
    let device = s.register(&mut conn, 1, Tier::B).await?;
    assert!(matches!(
        s.issue(&mut conn, device, 1, 100_001).await?,
        Err(OfflineError::OverTierLimit { max_minor: 100_000 })
    ));

    // A client may lower a limit, never raise it.
    let mut admin = s.db.admin().await?;
    let lower = TierLimits {
        max_voucher_minor: 50_000,
        lifetime_micros: 86_400_000_000,
    };
    set_tier_limits(&mut admin, s.client, Tier::B, Some(lower)).await?;
    assert!(matches!(
        s.issue(&mut conn, device, 2, 50_001).await?,
        Err(OfflineError::OverTierLimit { max_minor: 50_000 })
    ));
    let higher = TierLimits {
        max_voucher_minor: 100_001,
        ..lower
    };
    assert!(matches!(
        set_tier_limits(&mut admin, s.client, Tier::B, Some(higher)).await,
        Err(OfflineError::Invalid(_))
    ));
    let issued = s.issue(&mut conn, device, 3, 50_000).await??;
    assert_eq!(
        issued.voucher.expires_at.as_micros() - NOW,
        lower.lifetime_micros
    );
    // Nothing moved for the refused requests.
    assert_eq!(balance(&mut conn, HANA).await?, 950_000);

    // Tier C is disabled until the client enables it.
    let mut c = NewDevice {
        id: id(150)?,
        client: s.client,
        owner_account: id(HANA)?,
        public_key: key("phone c")?.public_key().clone(),
        attestation: Attestation {
            kind: AttestationKind::None,
            tier: Tier::C,
            detail: String::new(),
        },
        label: "phone".into(),
        now: Timestamp::from_micros(NOW),
    };
    assert!(matches!(
        register_device(&mut conn, &s.issuer, c.clone()).await,
        Err(OfflineError::TierDisabled(Tier::C))
    ));
    set_tier_limits(&mut admin, s.client, Tier::C, Some(Tier::C.maximum())).await?;
    let device_c = register_device(&mut conn, &s.issuer, c.clone()).await?;
    assert!(matches!(device_c, Registered::New(_)));
    assert!(matches!(
        s.issue(&mut conn, device_c.device().id(), 4, 20_001)
            .await?,
        Err(OfflineError::OverTierLimit { max_minor: 20_000 })
    ));

    // Disabled again: no new vouchers for its devices.
    set_tier_limits(&mut admin, s.client, Tier::C, None).await?;
    assert!(matches!(
        s.issue(&mut conn, device_c.device().id(), 5, 100).await?,
        Err(OfflineError::TierDisabled(Tier::C))
    ));

    // The same key with the same details is the same device; with others,
    // refused.
    c.id = id(151)?;
    set_tier_limits(&mut admin, s.client, Tier::C, Some(Tier::C.maximum())).await?;
    assert!(matches!(
        register_device(&mut conn, &s.issuer, c.clone()).await?,
        Registered::Repeated(d) if d.id() == device_c.device().id()
    ));
    c.label = "another".into();
    assert!(matches!(
        register_device(&mut conn, &s.issuer, c).await,
        Err(OfflineError::KeyRegistered)
    ));
    drop((conn, admin));
    s.db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn frozen_and_revoked_devices_get_no_voucher() -> TestResult {
    let Some(s) = setup(1_000_000).await? else {
        return Ok(());
    };
    let mut conn = s.db.app().await?;
    let device = s.register(&mut conn, 1, Tier::A).await?;

    let frozen = set_device_status(
        &mut conn,
        Some(s.client),
        device,
        DeviceStatus::Frozen,
        Some("conflict"),
    )
    .await?;
    assert_eq!(frozen.status, DeviceStatus::Frozen);
    assert!(matches!(
        s.issue(&mut conn, device, 1, 1_000).await?,
        Err(OfflineError::DeviceNotActive(DeviceStatus::Frozen))
    ));

    // The operator lifts the freeze, and it works again.
    set_device_status(&mut conn, None, device, DeviceStatus::Active, None).await?;
    s.issue(&mut conn, device, 2, 1_000).await??;
    // One open voucher per device.
    assert!(matches!(
        s.issue(&mut conn, device, 3, 1_000).await?,
        Err(OfflineError::VoucherOpen)
    ));

    let revoked = set_device_status(
        &mut conn,
        Some(s.client),
        device,
        DeviceStatus::Revoked,
        None,
    )
    .await?;
    assert_eq!(revoked.status, DeviceStatus::Revoked);
    // Revoking again changes nothing; reviving is refused.
    set_device_status(
        &mut conn,
        Some(s.client),
        device,
        DeviceStatus::Revoked,
        None,
    )
    .await?;
    assert!(matches!(
        set_device_status(
            &mut conn,
            Some(s.client),
            device,
            DeviceStatus::Active,
            None
        )
        .await,
        Err(OfflineError::BadTransition { .. })
    ));
    // Another client cannot see the device.
    let other: ClientId = id(0xbad)?;
    assert!(matches!(
        set_device_status(&mut conn, Some(other), device, DeviceStatus::Frozen, None).await,
        Err(OfflineError::DeviceNotFound)
    ));

    // A device must belong to an open customer wallet.
    let not_a_wallet = NewDevice {
        id: id(160)?,
        client: s.client,
        owner_account: id(BANK)?,
        public_key: key("card x")?.public_key().clone(),
        attestation: Attestation {
            kind: AttestationKind::Card,
            tier: Tier::A,
            detail: String::new(),
        },
        label: "card".into(),
        now: Timestamp::from_micros(NOW),
    };
    assert!(matches!(
        register_device(&mut conn, &s.issuer, not_a_wallet).await,
        Err(OfflineError::NotAWallet(_))
    ));
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop(conn);
    s.db.drop().await?;
    Ok(())
}

/// Outstanding escrow per customer is capped at 5,000 ETB across devices.
#[tokio::test]
async fn the_customer_cap_holds_across_devices() -> TestResult {
    let Some(s) = setup(10_000_000).await? else {
        return Ok(());
    };
    let mut conn = s.db.app().await?;
    for n in 1..=3 {
        let device = s.register(&mut conn, n, Tier::A).await?;
        let result = s.issue(&mut conn, device, n, 200_000).await?;
        if n < 3 {
            result?;
        } else {
            assert!(matches!(
                result,
                Err(OfflineError::OverCustomerCap {
                    outstanding_minor: 400_000,
                    cap_minor: 500_000
                })
            ));
        }
    }
    drop(conn);
    s.db.drop().await?;
    Ok(())
}

/// Many devices of one customer issue at once: neither the wallet nor the
/// cap is exceeded, and every escrow holds exactly its voucher.
#[tokio::test]
async fn concurrent_issues_cannot_exceed_the_wallet_or_the_cap() -> TestResult {
    // 300 ETB: room for three vouchers of 100.
    let Some(s) = setup(30_000).await? else {
        return Ok(());
    };
    let s = std::sync::Arc::new(s);
    let pool = s.db.app_pool_of(10).await?;
    let mut devices = Vec::new();
    {
        let mut conn = pool.acquire().await?;
        for n in 1..=10 {
            devices.push(s.register(&mut conn, n, Tier::B).await?);
        }
    }
    let mut tasks = Vec::new();
    for (n, device) in devices.iter().copied().enumerate() {
        let (s, pool) = (s.clone(), pool.clone());
        tasks.push(tokio::spawn(async move {
            let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
            s.issue(&mut conn, device, n as u64, 10_000)
                .await
                .map_err(|e| e.to_string())
        }));
    }
    let mut issued = 0;
    for task in tasks {
        match task.await?? {
            Ok(_) => issued += 1,
            Err(OfflineError::Ledger(LedgerError::Posting(PostingError::InsufficientFunds {
                ..
            }))) => {}
            Err(other) => return Err(format!("unexpected: {other}").into()),
        }
    }
    let mut conn = pool.acquire().await?;
    assert_eq!(issued, 3);
    assert_eq!(balance(&mut conn, HANA).await?, 0);
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop((conn, pool));
    std::sync::Arc::try_unwrap(s)
        .map_err(|_| "setup still shared")?
        .db
        .drop()
        .await?;
    Ok(())
}

/// The cap under concurrency: five cards of 2,000 ETB at once, with plenty
/// in the wallet, give exactly two vouchers (4,000 of the 5,000 cap).
#[tokio::test]
async fn concurrent_issues_respect_the_customer_cap() -> TestResult {
    let Some(s) = setup(10_000_000).await? else {
        return Ok(());
    };
    let s = std::sync::Arc::new(s);
    let pool = s.db.app_pool_of(5).await?;
    let mut devices = Vec::new();
    {
        let mut conn = pool.acquire().await?;
        for n in 1..=5 {
            devices.push(s.register(&mut conn, n, Tier::A).await?);
        }
    }
    let mut tasks = Vec::new();
    for (n, device) in devices.iter().copied().enumerate() {
        let (s, pool) = (s.clone(), pool.clone());
        tasks.push(tokio::spawn(async move {
            let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
            s.issue(&mut conn, device, n as u64, 200_000)
                .await
                .map_err(|e| e.to_string())
        }));
    }
    let mut issued = 0;
    for task in tasks {
        match task.await?? {
            Ok(_) => issued += 1,
            Err(OfflineError::OverCustomerCap { .. }) => {}
            Err(other) => return Err(format!("unexpected: {other}").into()),
        }
    }
    let mut conn = pool.acquire().await?;
    assert_eq!(issued, 2);
    assert_eq!(balance(&mut conn, HANA).await?, 10_000_000 - 400_000);
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop((conn, pool));
    std::sync::Arc::try_unwrap(s)
        .map_err(|_| "setup still shared")?
        .db
        .drop()
        .await?;
    Ok(())
}

/// The invariant check finds an escrow that disagrees with its voucher.
#[tokio::test]
async fn escrow_findings_catch_money_without_a_voucher() -> TestResult {
    let Some(s) = setup(1_000_000).await? else {
        return Ok(());
    };
    let mut store = PgStore::new(s.db.app_pool().await?, s.client);
    let stray: AccountId = id(777)?;
    store
        .open_account(stray, &s.chart, "2050:vch-stray")
        .await?;
    store
        .post(
            id::<EntryId>(778)?,
            EntryDraft::default()
                .debit(id(HANA)?, etb(500))
                .credit(stray, etb(500)),
        )
        .await?;
    let mut conn = s.db.app().await?;
    let findings = escrow_findings(&mut conn).await?;
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].escrow_account, stray);
    assert_eq!(findings[0].voucher, None);
    assert_eq!(
        (findings[0].expected_minor, findings[0].actual_minor),
        (0, 500)
    );
    drop((conn, store));
    s.db.drop().await?;
    Ok(())
}

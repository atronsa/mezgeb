//! Offline devices and vouchers on PostgreSQL (step 2.11, ADR-0019,
//! `docs/offline/protocol.md` §4–§7, migration 0011).
//!
//! - [`register_device`] records a device's P-256 key and tier and signs its
//!   certificate with the issuer key. Registering the same key again with
//!   the same details returns the device: the key is the idempotency key.
//! - [`issue_offline_voucher`] checks the device and its limits, opens the voucher's
//!   own escrow `2050:vch-…`, posts E16 (booked by the caller's template),
//!   and signs the voucher, in one transaction. The device's row lock makes
//!   one voucher at a time per device; the owner's wallet lock makes the
//!   per-customer cap hold across their devices.
//! - [`set_device_status`] freezes, reports lost, reactivates or revokes.
//! - [`escrow_findings`] checks that each escrow holds exactly its unspent
//!   voucher value.
//!
//! The engine knows nothing about templates: the caller books E16 with
//! its template through the `book` function, and the engine checks
//! that the draft moves exactly the voucher's amount from the wallet into
//! the new escrow.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use mezgeb_core::chain::hex;
use mezgeb_core::{
    Account, AccountId, Chart, ClientId, Currency, DeviceId, EntryDraft, EntryId, IdempotencyKey,
    LedgerError, Money, PositiveAmount, Side, Timestamp, VoucherId, voucher_issue_fingerprint,
};
use mezgeb_offline::{
    CERTIFICATE_MICROS, CUSTOMER_CAP_MINOR, DeviceCertificate, PublicKey, SecretKey, Tier,
    TierLimits, Voucher,
};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::concurrency::PostOptions;
use crate::post::{Claim, PostRequest, claim_key, entry_exists, post_in_transaction};
use crate::rows::{account_id, constraint_violation, db, entry_id, storage};

/// The ledger's P-256 issuer key (protocol §4): it signs device
/// certificates and vouchers.
///
/// A 32-byte scalar in hex, read from a file the operator controls
/// (`MEZGEB_OFFLINE_ISSUER_KEY_FILE`); step 3.12 moves it to a key service.
/// Only its public half is stored, in `offline_issuer_keys`.
#[derive(Debug, Clone)]
pub struct IssuerKey {
    key: SecretKey,
}

impl IssuerKey {
    pub fn from_secret(key: SecretKey) -> IssuerKey {
        IssuerKey { key }
    }

    /// A key from its scalar in hex (64 digits).
    pub fn from_hex(text: &str) -> Result<IssuerKey, String> {
        let text = text.trim();
        let bytes: Option<Vec<u8>> = (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
            .collect();
        let scalar: [u8; 32] = bytes
            .filter(|_| text.len() == 64)
            .and_then(|b| b.try_into().ok())
            .ok_or("the issuer key must be 64 hex digits")?;
        let key = SecretKey::from_bytes(&scalar).map_err(|e| e.to_string())?;
        Ok(IssuerKey { key })
    }

    /// A key from a file holding its scalar in hex.
    pub fn from_file(path: &Path) -> Result<IssuerKey, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read the issuer key {}: {e}", path.display()))?;
        IssuerKey::from_hex(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A new random key's scalar, in hex, for `mezgeb-cli key generate
    /// --offline-issuer`.
    pub fn generate_hex() -> Result<String, LedgerError> {
        loop {
            let mut scalar = [0u8; 32];
            getrandom::fill(&mut scalar).map_err(|e| storage(format!("no randomness: {e}")))?;
            // All but about 2^-32 of 256-bit values are valid scalars.
            if SecretKey::from_bytes(&scalar).is_ok() {
                return Ok(hex(&scalar));
            }
        }
    }

    pub fn key_id(&self) -> String {
        self.key.public_key().key_id()
    }

    pub fn public_key(&self) -> &PublicKey {
        self.key.public_key()
    }

    /// Records the public key, once, so its signatures can be verified.
    pub async fn register(&self, conn: &mut PgConnection) -> Result<(), LedgerError> {
        sqlx::query(
            "INSERT INTO offline_issuer_keys (key_id, public_key) VALUES ($1, $2)
             ON CONFLICT (key_id) DO NOTHING",
        )
        .bind(self.key_id())
        .bind(&self.public_key().compressed()[..])
        .execute(&mut *conn)
        .await
        .map_err(db)?;
        Ok(())
    }
}

/// Every issuer key ever used, oldest first: its ID, compressed public
/// key, and when it was added, in µs. Devices and payees verify vouchers
/// and certificates with them.
pub async fn issuer_keys(
    conn: &mut PgConnection,
) -> Result<Vec<(String, Vec<u8>, Timestamp)>, LedgerError> {
    let rows = sqlx::query(
        "SELECT key_id, public_key, (extract(epoch FROM added_at) * 1000000)::bigint AS added
           FROM offline_issuer_keys ORDER BY added_at, key_id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    rows.iter()
        .map(|r| {
            Ok((
                r.try_get("key_id").map_err(db)?,
                r.try_get("public_key").map_err(db)?,
                Timestamp::from_micros(r.try_get("added").map_err(db)?),
            ))
        })
        .collect()
}

/// Why an offline request was refused.
#[derive(Debug)]
pub enum OfflineError {
    /// No such device for this client.
    DeviceNotFound,
    /// No such voucher for this client.
    VoucherNotFound,
    /// The public key is registered to another device or with other details.
    KeyRegistered,
    /// The owner account is not an open customer wallet (2010).
    NotAWallet(AccountId),
    /// The client has not enabled this tier.
    TierDisabled(Tier),
    /// Only an `active` device gets a voucher.
    DeviceNotActive(DeviceStatus),
    /// The device's certificate has expired.
    CertificateExpired,
    /// The device already has an open voucher (ADR-0019 decision 3).
    VoucherOpen,
    /// Vouchers are in ETB, the currency of the tier limits.
    WrongCurrency(Currency),
    /// More than the tier's per-voucher limit, in minor units.
    OverTierLimit {
        max_minor: i64,
    },
    /// The owner's open vouchers would exceed the per-customer cap.
    OverCustomerCap {
        outstanding_minor: i64,
        cap_minor: i64,
    },
    /// A device cannot move from one status to the other.
    BadTransition {
        from: DeviceStatus,
        to: DeviceStatus,
    },
    /// Limits above the protocol's maxima, or a field the database refuses.
    Invalid(String),
    /// The booking function failed, or booked something other than E16.
    Booking(String),
    Ledger(LedgerError),
}

impl fmt::Display for OfflineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OfflineError::DeviceNotFound => f.write_str("no such device"),
            OfflineError::VoucherNotFound => f.write_str("no such voucher"),
            OfflineError::KeyRegistered => {
                f.write_str("this public key is already registered with other details")
            }
            OfflineError::NotAWallet(id) => {
                write!(f, "account {id} is not an open customer wallet")
            }
            OfflineError::TierDisabled(tier) => write!(f, "tier {tier} is not enabled"),
            OfflineError::DeviceNotActive(status) => {
                write!(f, "the device is {}, not active", status.as_str())
            }
            OfflineError::CertificateExpired => f.write_str("the device certificate has expired"),
            OfflineError::VoucherOpen => f.write_str("the device already has an open voucher"),
            OfflineError::WrongCurrency(c) => write!(f, "vouchers are in ETB, not {}", c.code()),
            OfflineError::OverTierLimit { max_minor } => {
                write!(
                    f,
                    "over the tier's voucher limit of {max_minor} minor units"
                )
            }
            OfflineError::OverCustomerCap {
                outstanding_minor,
                cap_minor,
            } => write!(
                f,
                "the customer has {outstanding_minor} minor units in open vouchers; the cap is {cap_minor}"
            ),
            OfflineError::BadTransition { from, to } => write!(
                f,
                "a device cannot go from {} to {}",
                from.as_str(),
                to.as_str()
            ),
            OfflineError::Invalid(why) | OfflineError::Booking(why) => f.write_str(why),
            OfflineError::Ledger(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OfflineError {}

impl From<LedgerError> for OfflineError {
    fn from(error: LedgerError) -> OfflineError {
        OfflineError::Ledger(error)
    }
}

pub(crate) fn sql(error: sqlx::Error) -> OfflineError {
    OfflineError::Ledger(db(error))
}

/// A device's status (protocol §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Active,
    /// A conflict; an operator decides.
    Frozen,
    /// Its owner reported it lost.
    Lost,
    /// Retired for good.
    Revoked,
}

impl DeviceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            DeviceStatus::Active => "active",
            DeviceStatus::Frozen => "frozen",
            DeviceStatus::Lost => "lost",
            DeviceStatus::Revoked => "revoked",
        }
    }

    /// Whether a device may move from `self` to `to`.
    pub fn may_become(self, to: DeviceStatus) -> bool {
        use DeviceStatus::{Active, Frozen, Lost, Revoked};
        matches!(
            (self, to),
            (Active, Frozen | Lost | Revoked) | (Frozen, Active | Revoked) | (Lost, Revoked)
        )
    }
}

impl FromStr for DeviceStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<DeviceStatus, String> {
        match s {
            "active" => Ok(DeviceStatus::Active),
            "frozen" => Ok(DeviceStatus::Frozen),
            "lost" => Ok(DeviceStatus::Lost),
            "revoked" => Ok(DeviceStatus::Revoked),
            other => Err(format!("unknown device status {other:?}")),
        }
    }
}

/// How the tier was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestationKind {
    /// No attestation: tier C, if the client enabled it.
    None,
    /// A card chip's certificate (tier A).
    Card,
    /// Android Key Attestation (tier B).
    Android,
}

impl AttestationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            AttestationKind::None => "none",
            AttestationKind::Card => "card",
            AttestationKind::Android => "android",
        }
    }
}

impl FromStr for AttestationKind {
    type Err = String;

    fn from_str(s: &str) -> Result<AttestationKind, String> {
        match s {
            "none" => Ok(AttestationKind::None),
            "card" => Ok(AttestationKind::Card),
            "android" => Ok(AttestationKind::Android),
            other => Err(format!("unknown attestation {other:?}")),
        }
    }
}

/// A verified attestation: what was checked and the tier it earns. The
/// caller verifies it; the engine records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub kind: AttestationKind,
    pub tier: Tier,
    /// What was checked, for the record.
    pub detail: String,
}

/// A device to register.
#[derive(Debug, Clone)]
pub struct NewDevice {
    pub id: DeviceId,
    pub client: ClientId,
    pub owner_account: AccountId,
    pub public_key: PublicKey,
    pub attestation: Attestation,
    pub label: String,
    /// Ledger time.
    pub now: Timestamp,
}

/// A registered device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub certificate: DeviceCertificate,
    pub certificate_signature: Vec<u8>,
    pub attestation: AttestationKind,
    pub attestation_detail: String,
    pub label: String,
    pub status: DeviceStatus,
    pub status_reason: Option<String>,
    /// The first counter its next voucher authorises.
    pub next_counter: u64,
}

impl Device {
    pub fn id(&self) -> DeviceId {
        self.certificate.device_id
    }
}

/// What [`register_device`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registered {
    New(Device),
    /// The same key was registered before with the same details.
    Repeated(Device),
}

impl Registered {
    pub fn device(&self) -> &Device {
        match self {
            Registered::New(d) | Registered::Repeated(d) => d,
        }
    }
}

const DEVICE_COLUMNS: &str = "id, client_id, owner_account_id, public_key, tier, attestation, \
     attestation_detail, label, issuer_key_id, certificate_signature, next_counter, status, \
     status_reason, (extract(epoch FROM issued_at) * 1000000)::bigint AS issued_micros, \
     (extract(epoch FROM valid_until) * 1000000)::bigint AS valid_until_micros";

fn device_from_row(row: &PgRow) -> Result<Device, LedgerError> {
    let get_uuid = |name: &str| row.try_get::<Uuid, _>(name).map_err(db);
    let get_text = |name: &str| row.try_get::<String, _>(name).map_err(db);
    let public_key: Vec<u8> = row.try_get("public_key").map_err(db)?;
    let next_counter: i64 = row.try_get("next_counter").map_err(db)?;
    Ok(Device {
        certificate: DeviceCertificate {
            device_id: DeviceId::from_uuid(get_uuid("id")?).map_err(storage)?,
            client_id: ClientId::from_uuid(get_uuid("client_id")?).map_err(storage)?,
            owner_account_id: account_id(get_uuid("owner_account_id")?)?,
            public_key: PublicKey::from_compressed(&public_key).map_err(storage)?,
            tier: get_text("tier")?.parse().map_err(storage)?,
            issued_at: Timestamp::from_micros(row.try_get("issued_micros").map_err(db)?),
            valid_until: Timestamp::from_micros(row.try_get("valid_until_micros").map_err(db)?),
            issuer_key_id: get_text("issuer_key_id")?,
        },
        certificate_signature: row.try_get("certificate_signature").map_err(db)?,
        attestation: get_text("attestation")?.parse().map_err(storage)?,
        attestation_detail: get_text("attestation_detail")?,
        label: get_text("label")?,
        status: get_text("status")?.parse().map_err(storage)?,
        status_reason: row.try_get("status_reason").map_err(db)?,
        next_counter: u64::try_from(next_counter).map_err(storage)?,
    })
}

async fn device_by_key(
    conn: &mut PgConnection,
    key: &PublicKey,
) -> Result<Option<Device>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DEVICE_COLUMNS} FROM devices WHERE public_key = $1"
    )))
    .bind(&key.compressed()[..])
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(device_from_row).transpose()
}

/// A device of `client`, optionally locked for update.
pub(crate) async fn load_device(
    conn: &mut PgConnection,
    client: Option<ClientId>,
    id: DeviceId,
    lock: bool,
) -> Result<Device, OfflineError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DEVICE_COLUMNS} FROM devices
          WHERE id = $1 AND ($2::uuid IS NULL OR client_id = $2){}",
        if lock { " FOR UPDATE" } else { "" }
    )))
    .bind(id.as_uuid())
    .bind(client.map(ClientId::as_uuid))
    .fetch_optional(&mut *conn)
    .await
    .map_err(sql)?;
    match row {
        Some(row) => Ok(device_from_row(&row)?),
        None => Err(OfflineError::DeviceNotFound),
    }
}

/// A device of `client`.
pub async fn get_device(
    conn: &mut PgConnection,
    client: ClientId,
    id: DeviceId,
) -> Result<Device, OfflineError> {
    load_device(conn, Some(client), id, false).await
}

/// Whether a repeat registration asks for what is registered.
fn same_registration(device: &Device, new: &NewDevice) -> bool {
    let c = &device.certificate;
    c.client_id == new.client
        && c.owner_account_id == new.owner_account
        && c.tier == new.attestation.tier
        && device.attestation == new.attestation.kind
        && device.label == new.label
}

/// The limits of `tier` for `client`, or `None` if the tier is disabled.
/// Without a row of its own, a client has the maxima for tiers A and B and
/// tier C disabled.
pub async fn tier_limits(
    conn: &mut PgConnection,
    client: ClientId,
    tier: Tier,
) -> Result<Option<TierLimits>, LedgerError> {
    let row = sqlx::query(
        "SELECT enabled, max_voucher_minor, lifetime_micros
           FROM offline_tier_limits WHERE client_id = $1 AND tier = $2",
    )
    .bind(client.as_uuid())
    .bind(tier.as_str())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    Ok(match row {
        None if tier == Tier::C => None,
        None => Some(tier.maximum()),
        Some(row) => {
            let enabled: bool = row.try_get("enabled").map_err(db)?;
            enabled
                .then(|| -> Result<TierLimits, LedgerError> {
                    Ok(TierLimits {
                        max_voucher_minor: row.try_get("max_voucher_minor").map_err(db)?,
                        lifetime_micros: row.try_get("lifetime_micros").map_err(db)?,
                    })
                })
                .transpose()?
        }
    })
}

/// Sets `client`'s limits for `tier`, at or below the maxima, or disables
/// the tier (`None`). For `mezgeb-cli offline tier`, as `mezgeb_admin`.
pub async fn set_tier_limits(
    conn: &mut PgConnection,
    client: ClientId,
    tier: Tier,
    limits: Option<TierLimits>,
) -> Result<(), OfflineError> {
    if let Some(l) = limits
        && !l.within(tier)
    {
        return Err(OfflineError::Invalid(format!(
            "tier {tier} limits must be positive and at most {} minor units and {} µs",
            tier.maximum().max_voucher_minor,
            tier.maximum().lifetime_micros
        )));
    }
    let stored = limits.unwrap_or(tier.maximum());
    sqlx::query(
        "INSERT INTO offline_tier_limits
             (client_id, tier, enabled, max_voucher_minor, lifetime_micros)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (client_id, tier) DO UPDATE
            SET enabled = EXCLUDED.enabled,
                max_voucher_minor = EXCLUDED.max_voucher_minor,
                lifetime_micros = EXCLUDED.lifetime_micros,
                updated_at = clock_timestamp()",
    )
    .bind(client.as_uuid())
    .bind(tier.as_str())
    .bind(limits.is_some())
    .bind(stored.max_voucher_minor)
    .bind(stored.lifetime_micros)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(())
}

/// The code and status of an account.
async fn account_kind(
    conn: &mut PgConnection,
    id: AccountId,
) -> Result<Option<(i16, String)>, LedgerError> {
    let row = sqlx::query("SELECT code, status FROM accounts WHERE id = $1")
        .bind(id.as_uuid())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db)?;
    row.map(|r| -> Result<_, LedgerError> {
        Ok((
            r.try_get("code").map_err(db)?,
            r.try_get("status").map_err(db)?,
        ))
    })
    .transpose()
}

/// Registers a device and signs its certificate (protocol §5).
///
/// The caller has verified the attestation, which decides the tier. The
/// owner account must be an open customer wallet, and the client must have
/// the tier enabled. The public key identifies the request: registering it
/// again with the same details returns the device; with other details it is
/// [`OfflineError::KeyRegistered`].
pub async fn register_device(
    conn: &mut PgConnection,
    issuer: &IssuerKey,
    new: NewDevice,
) -> Result<Registered, OfflineError> {
    let repeat = |device: Device| {
        if same_registration(&device, &new) {
            Ok(Registered::Repeated(device))
        } else {
            Err(OfflineError::KeyRegistered)
        }
    };
    if let Some(device) = device_by_key(conn, &new.public_key).await? {
        return repeat(device);
    }
    match account_kind(conn, new.owner_account).await? {
        Some((2010, status)) if status == "open" => {}
        _ => return Err(OfflineError::NotAWallet(new.owner_account)),
    }
    let tier = new.attestation.tier;
    if tier_limits(conn, new.client, tier).await?.is_none() {
        return Err(OfflineError::TierDisabled(tier));
    }
    if new.label.is_empty() || new.label.chars().count() > 100 {
        return Err(OfflineError::Invalid(
            "a label is 1 to 100 characters".into(),
        ));
    }
    let certificate = DeviceCertificate {
        device_id: new.id,
        client_id: new.client,
        owner_account_id: new.owner_account,
        public_key: new.public_key.clone(),
        tier,
        issued_at: new.now,
        valid_until: Timestamp::from_micros(new.now.as_micros() + CERTIFICATE_MICROS),
        issuer_key_id: issuer.key_id(),
    };
    let signature = certificate.sign(&issuer.key);

    issuer.register(conn).await?;
    let inserted = sqlx::query(
        "INSERT INTO devices
             (id, client_id, owner_account_id, public_key, key_id, tier, attestation,
              attestation_detail, label, issued_at, valid_until, issuer_key_id,
              certificate_signature)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                 timestamptz 'epoch' + $10::bigint * interval '1 microsecond',
                 timestamptz 'epoch' + $11::bigint * interval '1 microsecond',
                 $12, $13)",
    )
    .bind(new.id.as_uuid())
    .bind(new.client.as_uuid())
    .bind(new.owner_account.as_uuid())
    .bind(&new.public_key.compressed()[..])
    .bind(new.public_key.key_id())
    .bind(tier.as_str())
    .bind(new.attestation.kind.as_str())
    .bind(&new.attestation.detail)
    .bind(&new.label)
    .bind(certificate.issued_at.as_micros())
    .bind(certificate.valid_until.as_micros())
    .bind(&certificate.issuer_key_id)
    .bind(&signature[..])
    .execute(&mut *conn)
    .await;
    match inserted {
        Ok(_) => Ok(Registered::New(
            load_device(conn, Some(new.client), new.id, false).await?,
        )),
        Err(error) => match constraint_violation(&error) {
            // A concurrent registration of the same key won.
            Some((code, constraint))
                if code == "23505"
                    && (constraint == "devices_public_key_key"
                        || constraint == "devices_key_id_key") =>
            {
                let device = device_by_key(conn, &new.public_key)
                    .await?
                    .ok_or_else(|| storage("a registered device disappeared"))?;
                repeat(device)
            }
            Some((code, _)) if code == "23505" => {
                Err(OfflineError::Invalid("this device ID is taken".into()))
            }
            _ => Err(sql(error)),
        },
    }
}

/// Moves a device to `to` (protocol §5): `active` → `frozen`, `lost` or
/// `revoked`; `frozen` → `active` or `revoked`; `lost` → `revoked`. Already
/// in `to`, it is returned unchanged. `client` limits it to one client's
/// devices; `None` is for operators.
pub async fn set_device_status(
    conn: &mut PgConnection,
    client: Option<ClientId>,
    id: DeviceId,
    to: DeviceStatus,
    reason: Option<&str>,
) -> Result<Device, OfflineError> {
    if reason.is_some_and(|r| r.chars().count() > 200) {
        return Err(OfflineError::Invalid(
            "a reason is at most 200 characters".into(),
        ));
    }
    let mut tx = sqlx::Acquire::begin(&mut *conn).await.map_err(sql)?;
    let device = load_device(&mut tx, client, id, true).await?;
    if device.status == to {
        tx.commit().await.map_err(sql)?;
        return Ok(device);
    }
    if !device.status.may_become(to) {
        return Err(OfflineError::BadTransition {
            from: device.status,
            to,
        });
    }
    sqlx::query(
        "UPDATE devices SET status = $2, status_reason = $3, status_changed_at = clock_timestamp()
          WHERE id = $1",
    )
    .bind(id.as_uuid())
    .bind(to.as_str())
    .bind(reason)
    .execute(&mut *tx)
    .await
    .map_err(sql)?;
    let device = load_device(&mut tx, client, id, false).await?;
    tx.commit().await.map_err(sql)?;
    Ok(device)
}

/// A voucher's status (protocol §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoucherStatus {
    Active,
    Grace,
    Frozen,
    Returned,
    Closed,
    Ended,
}

impl VoucherStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            VoucherStatus::Active => "active",
            VoucherStatus::Grace => "grace",
            VoucherStatus::Frozen => "frozen",
            VoucherStatus::Returned => "returned",
            VoucherStatus::Closed => "closed",
            VoucherStatus::Ended => "ended",
        }
    }

    /// Whether its escrow still holds money: `active`, `grace` or `frozen`.
    pub const fn is_open(self) -> bool {
        matches!(
            self,
            VoucherStatus::Active | VoucherStatus::Grace | VoucherStatus::Frozen
        )
    }
}

impl FromStr for VoucherStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<VoucherStatus, String> {
        match s {
            "active" => Ok(VoucherStatus::Active),
            "grace" => Ok(VoucherStatus::Grace),
            "frozen" => Ok(VoucherStatus::Frozen),
            "returned" => Ok(VoucherStatus::Returned),
            "closed" => Ok(VoucherStatus::Closed),
            "ended" => Ok(VoucherStatus::Ended),
            other => Err(format!("unknown voucher status {other:?}")),
        }
    }
}

/// A voucher to issue. The caller mints the IDs.
#[derive(Debug, Clone)]
pub struct NewVoucher {
    pub voucher_id: VoucherId,
    pub device_id: DeviceId,
    pub client: ClientId,
    /// The ID of E16, the entry that funds it.
    pub entry_id: EntryId,
    /// The ID of its new escrow account, `2050:vch-<voucher_id>`.
    pub escrow_account: AccountId,
    pub idempotency_key: IdempotencyKey,
    pub amount: PositiveAmount,
    /// Ledger time.
    pub now: Timestamp,
}

/// An issued voucher, as signed, with its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedVoucher {
    pub voucher: Voucher,
    pub signature: Vec<u8>,
    pub owner_account: AccountId,
    pub entry_id: EntryId,
    pub status: VoucherStatus,
}

pub(crate) const VOUCHER_COLUMNS: &str = "id, device_id, client_id, owner_account_id, escrow_account_id, \
     currency, amount_minor, counter_start, issuer_key_id, signature, entry_id, status, \
     (extract(epoch FROM issued_at) * 1000000)::bigint AS issued_micros, \
     (extract(epoch FROM expires_at) * 1000000)::bigint AS expires_micros";

pub(crate) fn voucher_from_row(row: &PgRow) -> Result<IssuedVoucher, LedgerError> {
    let get_uuid = |name: &str| row.try_get::<Uuid, _>(name).map_err(db);
    let currency: String = row.try_get("currency").map_err(db)?;
    let counter_start: i64 = row.try_get("counter_start").map_err(db)?;
    let status: String = row.try_get("status").map_err(db)?;
    Ok(IssuedVoucher {
        voucher: Voucher {
            voucher_id: VoucherId::from_uuid(get_uuid("id")?).map_err(storage)?,
            device_id: DeviceId::from_uuid(get_uuid("device_id")?).map_err(storage)?,
            client_id: ClientId::from_uuid(get_uuid("client_id")?).map_err(storage)?,
            escrow_account_id: account_id(get_uuid("escrow_account_id")?)?,
            amount: Money::from_minor(
                row.try_get("amount_minor").map_err(db)?,
                Currency::from_code(&currency).map_err(storage)?,
            ),
            counter_start: u64::try_from(counter_start).map_err(storage)?,
            issued_at: Timestamp::from_micros(row.try_get("issued_micros").map_err(db)?),
            expires_at: Timestamp::from_micros(row.try_get("expires_micros").map_err(db)?),
            issuer_key_id: row.try_get("issuer_key_id").map_err(db)?,
        },
        signature: row.try_get("signature").map_err(db)?,
        owner_account: account_id(get_uuid("owner_account_id")?)?,
        entry_id: entry_id(get_uuid("entry_id")?)?,
        status: status.parse().map_err(storage)?,
    })
}

/// A voucher of `client`.
pub async fn get_voucher(
    conn: &mut PgConnection,
    client: ClientId,
    id: VoucherId,
) -> Result<IssuedVoucher, OfflineError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {VOUCHER_COLUMNS} FROM vouchers WHERE id = $1 AND client_id = $2"
    )))
    .bind(id.as_uuid())
    .bind(client.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(sql)?;
    match row {
        Some(row) => Ok(voucher_from_row(&row)?),
        None => Err(OfflineError::VoucherNotFound),
    }
}

async fn voucher_of_entry(
    conn: &mut PgConnection,
    entry: EntryId,
) -> Result<Option<IssuedVoucher>, LedgerError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {VOUCHER_COLUMNS} FROM vouchers WHERE entry_id = $1"
    )))
    .bind(entry.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    row.as_ref().map(voucher_from_row).transpose()
}

/// Checks that `draft` is E16 for this voucher: exactly the amount, from
/// the wallet into the escrow, tagged with the voucher.
fn check_booking(
    draft: &EntryDraft,
    wallet: AccountId,
    escrow: AccountId,
    new: &NewVoucher,
) -> Result<(), OfflineError> {
    let amount = new.amount.money();
    let lines: Vec<(AccountId, Side, Money)> = draft
        .lines
        .iter()
        .map(|l| (l.account, l.side, l.amount))
        .collect();
    let expected = [
        (wallet, Side::Debit, amount),
        (escrow, Side::Credit, amount),
    ];
    if lines != expected {
        return Err(OfflineError::Booking(
            "the booking does not move the voucher's amount from the wallet into its escrow".into(),
        ));
    }
    if draft.metadata.get("voucher") != Some(&new.voucher_id.to_string()) {
        return Err(OfflineError::Booking(
            "the booking does not name the voucher".into(),
        ));
    }
    Ok(())
}

/// Opens the voucher's escrow `2050:vch-<voucher_id>`, owned by the client.
async fn open_escrow(
    conn: &mut PgConnection,
    chart: &Chart,
    new: &NewVoucher,
) -> Result<Account, OfflineError> {
    let reference = format!("2050:vch-{}", new.voucher_id);
    let account =
        Account::open(new.escrow_account, chart, &reference).map_err(LedgerError::Chart)?;
    sqlx::query(
        "INSERT INTO accounts
             (id, code, sub_account, name, account_type, currency, status, allow_negative,
              opened_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(account.id().as_uuid())
    .bind(i16::try_from(account.code().value()).map_err(storage)?)
    .bind(account.sub_account())
    .bind(account.name())
    .bind(account.account_type().as_str())
    .bind(account.currency().code())
    .bind(account.status().as_str())
    .bind(account.allow_negative())
    .bind(new.client.as_uuid())
    .execute(&mut *conn)
    .await
    .map_err(|e| match constraint_violation(&e) {
        Some((code, _)) if code == "23505" => {
            OfflineError::Ledger(LedgerError::DuplicateReference(account.reference()))
        }
        _ => sql(e),
    })?;
    Ok(account)
}

/// One attempt: [`issue_in`] in a transaction, committed if it succeeds and
/// rolled back at once if not, so a refusal releases the device's lock
/// before it returns.
async fn issue_once<F>(
    conn: &mut PgConnection,
    issuer: &IssuerKey,
    chart: &Chart,
    new: &NewVoucher,
    book: &F,
    options: &PostOptions,
) -> Result<IssuedVoucher, OfflineError>
where
    F: Fn(&BTreeMap<AccountId, Account>, AccountId, AccountId) -> Result<EntryDraft, String>,
{
    let mut tx = options.begin(conn).await?;
    match issue_in(&mut tx, issuer, chart, new, book, options).await {
        Ok(issued) => {
            options.commit(tx).await?;
            Ok(issued)
        }
        Err(error) => {
            // The refusal matters more than a failed rollback, which the
            // connection's next use completes anyway.
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

async fn issue_in<F>(
    tx: &mut PgConnection,
    issuer: &IssuerKey,
    chart: &Chart,
    new: &NewVoucher,
    book: &F,
    options: &PostOptions,
) -> Result<IssuedVoucher, OfflineError>
where
    F: Fn(&BTreeMap<AccountId, Account>, AccountId, AccountId) -> Result<EntryDraft, String>,
{
    let fingerprint = voucher_issue_fingerprint(new.device_id, new.amount);
    if entry_exists(tx, new.entry_id).await? {
        return Err(LedgerError::DuplicateEntry(new.entry_id).into());
    }
    let claim = PostRequest::new(
        new.entry_id,
        new.client,
        new.idempotency_key.clone(),
        EntryDraft::default(),
    );
    if let Claim::Existing(entry) = claim_key(tx, &claim, &fingerprint).await? {
        return voucher_of_entry(tx, entry.id())
            .await?
            .ok_or(OfflineError::Ledger(LedgerError::IdempotencyKeyReused));
    }

    // The device, locked: one issue at a time per device.
    let device = load_device(tx, Some(new.client), new.device_id, true).await?;
    if device.status != DeviceStatus::Active {
        return Err(OfflineError::DeviceNotActive(device.status));
    }
    if device.certificate.valid_until <= new.now {
        return Err(OfflineError::CertificateExpired);
    }
    let amount = new.amount.money();
    if amount.currency() != Currency::ETB {
        return Err(OfflineError::WrongCurrency(amount.currency()));
    }
    let tier = device.certificate.tier;
    let limits = tier_limits(tx, new.client, tier)
        .await?
        .ok_or(OfflineError::TierDisabled(tier))?;
    if amount.amount() > limits.max_voucher_minor {
        return Err(OfflineError::OverTierLimit {
            max_minor: limits.max_voucher_minor,
        });
    }
    let open: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM vouchers
                         WHERE device_id = $1 AND status IN ('active', 'grace', 'frozen'))",
    )
    .bind(new.device_id.as_uuid())
    .fetch_one(&mut *tx)
    .await
    .map_err(sql)?;
    if open {
        return Err(OfflineError::VoucherOpen);
    }

    // The owner's wallet, locked: one issue at a time per customer, so the
    // cap holds across their devices.
    let wallet = device.certificate.owner_account_id;
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR NO KEY UPDATE")
        .bind(wallet.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(sql)?;
    let outstanding: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM vouchers
          WHERE owner_account_id = $1 AND status IN ('active', 'grace', 'frozen')",
    )
    .bind(wallet.as_uuid())
    .fetch_one(&mut *tx)
    .await
    .map_err(sql)?;
    if outstanding.saturating_add(amount.amount()) > CUSTOMER_CAP_MINOR {
        return Err(OfflineError::OverCustomerCap {
            outstanding_minor: outstanding,
            cap_minor: CUSTOMER_CAP_MINOR,
        });
    }

    // E16, into a new escrow.
    let escrow = open_escrow(tx, chart, new).await?;
    let mut view = BTreeMap::new();
    let wallet_account = crate::accounts::get_account(tx, wallet)
        .await?
        .ok_or(LedgerError::AccountNotFound(wallet))?;
    view.insert(wallet, wallet_account);
    view.insert(escrow.id(), escrow);
    let draft = book(&view, wallet, new.escrow_account).map_err(OfflineError::Booking)?;
    check_booking(&draft, wallet, new.escrow_account, new)?;
    let posting = PostRequest::new(new.entry_id, new.client, new.idempotency_key.clone(), draft);
    post_in_transaction(tx, posting, None, options).await?;

    // The voucher, signed.
    let voucher = Voucher {
        voucher_id: new.voucher_id,
        device_id: new.device_id,
        client_id: new.client,
        escrow_account_id: new.escrow_account,
        amount,
        counter_start: device.next_counter,
        issued_at: new.now,
        expires_at: Timestamp::from_micros(new.now.as_micros() + limits.lifetime_micros),
        issuer_key_id: issuer.key_id(),
    };
    let signature = voucher.sign(&issuer.key);
    issuer.register(tx).await?;
    sqlx::query(
        "INSERT INTO vouchers
             (id, device_id, client_id, owner_account_id, escrow_account_id, currency,
              amount_minor, counter_start, issued_at, expires_at, issuer_key_id, signature,
              entry_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                 timestamptz 'epoch' + $9::bigint * interval '1 microsecond',
                 timestamptz 'epoch' + $10::bigint * interval '1 microsecond',
                 $11, $12, $13)",
    )
    .bind(new.voucher_id.as_uuid())
    .bind(new.device_id.as_uuid())
    .bind(new.client.as_uuid())
    .bind(wallet.as_uuid())
    .bind(new.escrow_account.as_uuid())
    .bind(amount.currency().code())
    .bind(amount.amount())
    .bind(i64::try_from(voucher.counter_start).map_err(storage)?)
    .bind(voucher.issued_at.as_micros())
    .bind(voucher.expires_at.as_micros())
    .bind(&voucher.issuer_key_id)
    .bind(&signature[..])
    .bind(new.entry_id.as_uuid())
    .execute(&mut *tx)
    .await
    .map_err(|e| match constraint_violation(&e) {
        Some((code, constraint))
            if code == "23505" && constraint == "vouchers_one_open_per_device" =>
        {
            OfflineError::VoucherOpen
        }
        _ => sql(e),
    })?;
    Ok(voucher_of_entry(tx, new.entry_id)
        .await?
        .ok_or_else(|| storage("an issued voucher disappeared"))?)
}

/// Issues a voucher (protocol §7): checks the device, its tier limit and
/// the per-customer cap; opens the escrow; posts E16, which `book` drafts
/// from the wallet, the escrow and a view of both; and signs the voucher.
/// All in one transaction, idempotent by the idempotency key.
pub async fn issue_offline_voucher<F>(
    conn: &mut PgConnection,
    issuer: &IssuerKey,
    chart: &Chart,
    new: &NewVoucher,
    book: F,
    options: &PostOptions,
) -> Result<IssuedVoucher, OfflineError>
where
    F: Fn(&BTreeMap<AccountId, Account>, AccountId, AccountId) -> Result<EntryDraft, String>,
{
    let mut attempt = 1;
    loop {
        match issue_once(conn, issuer, chart, new, &book, options).await {
            Err(OfflineError::Ledger(error)) if options.retry_after(&error, attempt).await => {
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// An escrow that does not hold its voucher's unspent value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscrowFinding {
    pub escrow_account: AccountId,
    /// `None` for a 2050 account with money and no voucher.
    pub voucher: Option<VoucherId>,
    pub expected_minor: i64,
    pub actual_minor: i64,
}

/// Checks the acceptance criterion of step 2.11: each voucher's escrow
/// holds exactly its unspent value (its amount less what its receipts took
/// from escrow while it is open, nothing once returned or closed), and no
/// 2050 account holds money without a voucher.
pub async fn escrow_findings(conn: &mut PgConnection) -> Result<Vec<EscrowFinding>, LedgerError> {
    let rows = sqlx::query(
        "SELECT a.id AS account, v.id AS voucher,
                CASE WHEN v.id IS NULL THEN 0
                     WHEN v.status IN ('active', 'grace', 'frozen')
                         THEN v.amount_minor - COALESCE((
                             SELECT sum(r.from_escrow_minor) FROM offline_receipts r
                              WHERE r.voucher_id = v.id), 0)
                     ELSE 0 END::bigint AS expected,
                a.credits_total - a.debits_total AS actual
           FROM accounts a
           LEFT JOIN vouchers v ON v.escrow_account_id = a.id
          WHERE a.code = 2050
          ORDER BY a.id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut findings = Vec::new();
    for row in rows {
        let expected: i64 = row.try_get("expected").map_err(db)?;
        let actual: i64 = row.try_get("actual").map_err(db)?;
        if expected != actual {
            let voucher: Option<Uuid> = row.try_get("voucher").map_err(db)?;
            findings.push(EscrowFinding {
                escrow_account: account_id(row.try_get("account").map_err(db)?)?,
                voucher: voucher
                    .map(VoucherId::from_uuid)
                    .transpose()
                    .map_err(storage)?,
                expected_minor: expected,
                actual_minor: actual,
            });
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_statuses_move_forward_only() {
        use DeviceStatus::{Active, Frozen, Lost, Revoked};
        assert!(Active.may_become(Frozen));
        assert!(Frozen.may_become(Active));
        assert!(Lost.may_become(Revoked));
        assert!(!Lost.may_become(Active));
        for to in [Active, Frozen, Lost] {
            assert!(!Revoked.may_become(to));
        }
        for s in [Active, Frozen, Lost, Revoked] {
            assert_eq!(s.as_str().parse(), Ok(s));
        }
    }

    #[test]
    fn issuer_keys_load_from_hex() {
        let hex = IssuerKey::generate_hex().expect("randomness");
        let key = IssuerKey::from_hex(&format!(" {hex}\n")).expect("a key");
        assert_eq!(key.key_id().len(), 16);
        assert!(IssuerKey::from_hex("abc").is_err());
        assert!(IssuerKey::from_hex(&"0".repeat(64)).is_err());
    }
}

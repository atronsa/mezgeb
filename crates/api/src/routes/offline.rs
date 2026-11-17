//! Offline (step 2.11, ADR-0019): register and revoke devices, issue
//! vouchers, publish the issuer keys. Receipts answer `501` until step 2.12.
//!
//! Attestation decides a device's tier. Until a card chip is chosen and
//! Android Key Attestation is verified, only `none` is accepted, which earns
//! tier C if the client has enabled it; `card` and `android` are refused as
//! `attestation_unsupported`.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::routing::{get, post};
use mezgeb_core::{AccountId, DeviceId, PositiveAmount, VoucherId};
use mezgeb_engine::{
    Attestation, AttestationKind, DeviceStatus, IssuerKey, NewDevice, NewVoucher, OfflineError,
    Registered, Upload,
};
use mezgeb_offline::{PublicKey, Tier};
use mezgeb_ops::OperationRequest;
use serde::Deserialize;
use serde_json::json;

use super::common::{ApiResult, bindable_account, connection, new_entry_id};
use crate::AppState;
use crate::auth::scope::{Auth, OfflineResolve, OfflineWrite};
use crate::http::extract::{IdemKey, JsonBody, parse_id};
use crate::http::reply::{json_response, written};
use crate::problem::ApiError;
use crate::wire::{
    DeviceJson, DisputeJson, IssuerKeyJson, MoneyJson, OfflineReceiptJson, ReceiptResultJson,
    ShortfallJson, VoucherJson, format_time, now, parse_hex,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/offline/devices", post(register_device))
        .route("/v1/offline/devices/{device_id}", get(get_device))
        .route(
            "/v1/offline/devices/{device_id}/revoke",
            post(revoke_device),
        )
        .route("/v1/offline/vouchers", post(issue_voucher))
        .route("/v1/offline/vouchers/{voucher_id}", get(get_voucher))
        .route("/v1/offline/issuer-keys", get(issuer_keys))
        .route(
            "/v1/offline/vouchers/{voucher_id}/receipts",
            get(voucher_receipts),
        )
        .route("/v1/offline/receipts", post(receipts))
        .route("/v1/offline/shortfalls", get(list_shortfalls))
        .route("/v1/offline/shortfalls/{shortfall_id}/repay", post(repay))
        .route("/v1/offline/disputes", get(list_disputes))
        .route("/v1/offline/disputes/{dispute_id}", get(get_dispute))
        .route("/v1/offline/disputes/{dispute_id}/resolve", post(resolve))
}

impl From<OfflineError> for ApiError {
    fn from(error: OfflineError) -> ApiError {
        let detail = error.to_string();
        match error {
            OfflineError::DeviceNotFound => ApiError::not_found("device"),
            OfflineError::VoucherNotFound => ApiError::not_found("voucher"),
            OfflineError::KeyRegistered => ApiError::new("public_key_registered", detail),
            OfflineError::NotAWallet(_) => ApiError::new("owner_not_a_wallet", detail),
            OfflineError::TierDisabled(_) => ApiError::new("tier_disabled", detail),
            OfflineError::DeviceNotActive(_) => ApiError::new("device_not_active", detail),
            OfflineError::CertificateExpired => ApiError::new("certificate_expired", detail),
            OfflineError::VoucherOpen => ApiError::new("voucher_open", detail),
            OfflineError::OverTierLimit { .. } => ApiError::new("over_tier_limit", detail),
            OfflineError::OverCustomerCap { .. } => ApiError::new("over_customer_cap", detail),
            OfflineError::WrongCurrency(_) => ApiError::invalid("/amount/currency", detail),
            OfflineError::Invalid(_) => ApiError::new("invalid_request", detail),
            OfflineError::NotYet(_) => ApiError::new("not_implemented", detail),
            OfflineError::ShortfallNotFound => ApiError::not_found("shortfall"),
            OfflineError::DisputeNotFound => ApiError::not_found("dispute"),
            OfflineError::ShortfallUnpaid { .. } => ApiError::new("shortfall_unpaid", detail),
            OfflineError::DisputeResolved(_) => ApiError::new("dispute_resolved", detail),
            OfflineError::BadTransition { .. } | OfflineError::Booking(_) => {
                ApiError::internal(detail)
            }
            OfflineError::Ledger(e) => ApiError::from(e),
        }
    }
}

fn issuer(state: &AppState) -> Result<Arc<IssuerKey>, ApiError> {
    state.issuer.clone().ok_or_else(|| {
        ApiError::new(
            "offline_unavailable",
            "This ledger has no offline issuer key, so offline payments are off.",
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttestationBody {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterDeviceBody {
    owner_account: String,
    public_key: String,
    label: String,
    attestation: AttestationBody,
}

/// The attestation, verified, and the tier it earns.
fn attestation(body: &AttestationBody) -> Result<Attestation, ApiError> {
    match body.kind.as_str() {
        "none" => Ok(Attestation {
            kind: AttestationKind::None,
            tier: Tier::C,
            detail: "no attestation".into(),
        }),
        "card" | "android" => Err(ApiError::new(
            "attestation_unsupported",
            format!(
                "{} attestation is not verified yet; register with \"none\" for tier C.",
                body.kind
            ),
        )),
        _ => Err(ApiError::invalid(
            "/attestation/type",
            "the attestation type is none, card or android",
        )),
    }
}

async fn register_device(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    JsonBody(body): JsonBody<RegisterDeviceBody>,
) -> ApiResult {
    let issuer = issuer(&state)?;
    let public_key = parse_hex(&body.public_key)
        .and_then(|bytes| PublicKey::from_compressed(&bytes).ok())
        .ok_or_else(|| {
            ApiError::invalid(
                "/public_key",
                "the public key is a compressed P-256 point: 66 hex digits",
            )
        })?;
    let attestation = attestation(&body.attestation)?;
    let mut conn = connection(&state).await?;
    let owner = bindable_account(&mut conn, &caller, &body.owner_account)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                "owner_not_a_wallet",
                format!("No customer wallet {:?}.", body.owner_account),
            )
        })?;
    drop(conn);
    let id = DeviceId::from_uuid(uuid::Uuid::now_v7()).map_err(ApiError::internal)?;
    let new = NewDevice {
        id,
        client: caller.client,
        owner_account: owner,
        public_key,
        attestation,
        label: body.label,
        now: now(),
    };
    let registered = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(mezgeb_engine::register_device(&mut conn, &issuer, new).await?)
            }
        })
        .await?;
    let device = registered.device();
    let mut response = written(
        matches!(registered, Registered::New(_)),
        DeviceJson::from(device),
        StatusCode::CREATED,
    );
    if let Ok(location) = HeaderValue::from_str(&format!("/v1/offline/devices/{}", device.id())) {
        response.headers_mut().insert("location", location);
    }
    Ok(response)
}

async fn get_device(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    Path(device_id): Path<String>,
) -> ApiResult {
    let id: DeviceId = parse_id(&device_id, "device_id")?;
    let mut conn = connection(&state).await?;
    let device = mezgeb_engine::get_device(&mut conn, caller.client, id).await?;
    Ok(json_response(StatusCode::OK, DeviceJson::from(&device)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RevokeBody {
    reason: Option<String>,
}

async fn revoke_device(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    Path(device_id): Path<String>,
    JsonBody(body): JsonBody<RevokeBody>,
) -> ApiResult {
    let id: DeviceId = parse_id(&device_id, "device_id")?;
    let device = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                Ok(mezgeb_engine::set_device_status(
                    &mut conn,
                    Some(caller.client),
                    id,
                    DeviceStatus::Revoked,
                    body.reason.as_deref(),
                )
                .await?)
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, DeviceJson::from(&device)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueVoucherBody {
    device: String,
    amount: MoneyJson,
    /// The device's last counter, as it reports it; 0 (the default) if it
    /// has never signed a receipt.
    #[serde(default)]
    last_counter: u64,
}

async fn issue_voucher(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    IdemKey(key): IdemKey,
    JsonBody(body): JsonBody<IssueVoucherBody>,
) -> ApiResult {
    let issuer = issuer(&state)?;
    let device: DeviceId = parse_id(&body.device, "device")?;
    let amount = PositiveAmount::new(body.amount.to_money("/amount")?)
        .map_err(|_| ApiError::invalid("/amount/amount", "the amount must be greater than zero"))?;
    let mint = || uuid::Uuid::now_v7();
    let new = NewVoucher {
        voucher_id: VoucherId::from_uuid(mint()).map_err(ApiError::internal)?,
        device_id: device,
        client: caller.client,
        entry_id: new_entry_id()?,
        escrow_account: AccountId::from_uuid(mint()).map_err(ApiError::internal)?,
        idempotency_key: key,
        amount,
        last_counter: body.last_counter,
        now: now(),
    };
    let issued = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let registry = state.registry.clone();
                let voucher = new.voucher_id.to_string();
                // E16, booked by its template.
                let book = move |view: &BTreeMap<AccountId, mezgeb_core::Account>,
                                 wallet: AccountId,
                                 escrow: AccountId| {
                    let request = OperationRequest {
                        template: "issue_voucher.v1".into(),
                        accounts: BTreeMap::from([
                            ("customer_wallet".into(), wallet),
                            ("voucher_escrow".into(), escrow),
                        ]),
                        amounts: BTreeMap::from([("amount".into(), amount.money())]),
                        metadata: BTreeMap::from([("voucher".into(), voucher.clone())]),
                        ..OperationRequest::default()
                    };
                    registry.draft(&request, view).map_err(|e| e.to_string())
                };
                Ok(mezgeb_engine::issue_offline_voucher(
                    &mut conn,
                    &issuer,
                    &state.chart,
                    &new,
                    book,
                    &state.options,
                )
                .await
                .map(|issued| {
                    let fresh = issued.voucher.voucher_id == new.voucher_id;
                    (issued, fresh)
                })?)
            }
        })
        .await?;
    let (voucher, fresh) = issued;
    let mut response = written(fresh, VoucherJson::from(&voucher), StatusCode::CREATED);
    if let Ok(location) = HeaderValue::from_str(&format!(
        "/v1/offline/vouchers/{}",
        voucher.voucher.voucher_id
    )) {
        response.headers_mut().insert("location", location);
    }
    Ok(response)
}

async fn get_voucher(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    Path(voucher_id): Path<String>,
) -> ApiResult {
    let id: VoucherId = parse_id(&voucher_id, "voucher_id")?;
    let mut conn = connection(&state).await?;
    let voucher = mezgeb_engine::get_voucher(&mut conn, caller.client, id).await?;
    Ok(json_response(StatusCode::OK, VoucherJson::from(&voucher)))
}

async fn issuer_keys(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<OfflineWrite>,
) -> ApiResult {
    let mut conn = connection(&state).await?;
    let items: Vec<IssuerKeyJson> = mezgeb_engine::issuer_keys(&mut conn)
        .await?
        .into_iter()
        .map(|(key_id, public, added)| IssuerKeyJson {
            key_id,
            public_key: mezgeb_core::chain::hex(&public),
            added_at: format_time(added),
        })
        .collect();
    Ok(json_response(StatusCode::OK, json!({ "items": items })))
}

/// The most receipts one upload may carry.
const MAX_RECEIPTS: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedReceiptBody {
    body: String,
    signature: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptsBody {
    receipts: Vec<SignedReceiptBody>,
}

/// Uploads receipts, from the payer's device or the payee's terminal: each
/// gets its own outcome (protocol §9). The batch is not one transaction:
/// each receipt is settled, kept or refused on its own, and an upload can
/// simply be repeated.
async fn receipts(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    JsonBody(body): JsonBody<ReceiptsBody>,
) -> ApiResult {
    if body.receipts.is_empty() || body.receipts.len() > MAX_RECEIPTS {
        return Err(ApiError::invalid(
            "/receipts",
            format!("an upload carries 1 to {MAX_RECEIPTS} receipts"),
        ));
    }
    let mut items = Vec::with_capacity(body.receipts.len());
    for (i, r) in body.receipts.iter().enumerate() {
        let decode = |text: &str, field: &str| {
            parse_hex(text)
                .ok_or_else(|| ApiError::invalid(&format!("/receipts/{i}/{field}"), "not hex"))
        };
        items.push((decode(&r.body, "body")?, decode(&r.signature, "signature")?));
    }
    let results = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                // The fee goes to 4010; what a payer cannot pay of a late
                // receipt, to 5030.
                let accounts = crate::offline_books::offline_accounts(&mut conn).await?;
                let books = crate::offline_books::TemplateBooks {
                    registry: state.registry.clone(),
                };
                let mut results = Vec::with_capacity(items.len());
                for (index, (body, signature)) in items.iter().enumerate() {
                    let upload = Upload {
                        client: caller.client,
                        body,
                        signature,
                        now: now(),
                    };
                    let result = mezgeb_engine::ingest_receipt(
                        &mut conn,
                        &upload,
                        accounts,
                        &books,
                        &state.options,
                    )
                    .await?;
                    results.push(ReceiptResultJson::from((index, &result)));
                }
                Ok(results)
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, json!({ "results": results })))
}

async fn voucher_receipts(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    Path(voucher_id): Path<String>,
) -> ApiResult {
    let id: VoucherId = parse_id(&voucher_id, "voucher_id")?;
    let mut conn = connection(&state).await?;
    // 404 for a voucher that is not the caller's.
    mezgeb_engine::get_voucher(&mut conn, caller.client, id).await?;
    let items: Vec<OfflineReceiptJson> =
        mezgeb_engine::voucher_receipts(&mut conn, caller.client, id)
            .await?
            .iter()
            .map(OfflineReceiptJson::from)
            .collect();
    Ok(json_response(StatusCode::OK, json!({ "items": items })))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ShortfallQuery {
    owner_account: Option<String>,
}

/// The client's customers' shortfalls: what late receipts' payers could not
/// pay, paid by the loss account until repaid.
async fn list_shortfalls(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    crate::http::extract::QueryParams(query): crate::http::extract::QueryParams<ShortfallQuery>,
) -> ApiResult {
    let mut conn = connection(&state).await?;
    let owner = match &query.owner_account {
        Some(text) => Some(
            bindable_account(&mut conn, &caller, text)
                .await?
                .ok_or_else(|| ApiError::not_found("account"))?,
        ),
        None => None,
    };
    let items: Vec<ShortfallJson> = mezgeb_engine::shortfalls(&mut conn, caller.client, owner)
        .await?
        .iter()
        .map(ShortfallJson::from)
        .collect();
    Ok(json_response(StatusCode::OK, json!({ "items": items })))
}

/// The payer repays a shortfall from their wallet (E30); their devices can
/// then get vouchers again.
async fn repay(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineWrite>,
    Path(shortfall_id): Path<String>,
) -> ApiResult {
    let id: uuid::Uuid = parse_id(&shortfall_id, "shortfall_id")?;
    let shortfall = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let accounts = crate::offline_books::offline_accounts(&mut conn).await?;
                let books = crate::offline_books::TemplateBooks {
                    registry: state.registry.clone(),
                };
                Ok(mezgeb_engine::repay_shortfall(
                    &mut conn,
                    caller.client,
                    id,
                    accounts,
                    &books,
                    &state.options,
                )
                .await?)
            }
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        ShortfallJson::from(&shortfall),
    ))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DisputeQuery {
    status: Option<String>,
}

/// Every client's offline disputes, newest first (operators).
async fn list_disputes(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<OfflineResolve>,
    crate::http::extract::QueryParams(query): crate::http::extract::QueryParams<DisputeQuery>,
) -> ApiResult {
    let open_only = match query.status.as_deref() {
        None | Some("all") => false,
        Some("open") => true,
        Some(_) => return Err(ApiError::invalid("/status", "the status is open or all")),
    };
    let mut conn = connection(&state).await?;
    let items: Vec<DisputeJson> = mezgeb_engine::disputes(&mut conn, None, open_only)
        .await?
        .iter()
        .map(DisputeJson::from)
        .collect();
    Ok(json_response(StatusCode::OK, json!({ "items": items })))
}

async fn get_dispute(
    State(state): State<AppState>,
    Auth(_caller, _): Auth<OfflineResolve>,
    Path(dispute_id): Path<String>,
) -> ApiResult {
    let id: uuid::Uuid = parse_id(&dispute_id, "dispute_id")?;
    let mut conn = connection(&state).await?;
    let dispute = mezgeb_engine::get_dispute(&mut conn, None, id).await?;
    Ok(json_response(StatusCode::OK, DisputeJson::from(&dispute)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    resolution: String,
}

/// Ends a dispute: `paid` pays every valid receipt by the loss policy,
/// `refused` none; the escrow left returns and the device is revoked.
async fn resolve(
    State(state): State<AppState>,
    Auth(caller, _): Auth<OfflineResolve>,
    Path(dispute_id): Path<String>,
    JsonBody(body): JsonBody<ResolveBody>,
) -> ApiResult {
    let id: uuid::Uuid = parse_id(&dispute_id, "dispute_id")?;
    let resolution = mezgeb_engine::DisputeResolution::parse(&body.resolution)
        .ok_or_else(|| ApiError::invalid("/resolution", "the resolution is paid or refused"))?;
    let actor = caller.actor.as_str().to_owned();
    let dispute = state
        .write({
            let state = state.clone();
            async move {
                let mut conn = connection(&state).await?;
                let accounts = crate::offline_books::offline_accounts(&mut conn).await?;
                let books = crate::offline_books::TemplateBooks {
                    registry: state.registry.clone(),
                };
                Ok(mezgeb_engine::resolve_dispute(
                    &mut conn,
                    id,
                    resolution,
                    &actor,
                    now(),
                    accounts,
                    &books,
                    &state.options,
                )
                .await?)
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, DisputeJson::from(&dispute)))
}

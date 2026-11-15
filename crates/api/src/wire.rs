//! The JSON the API sends and receives (`openapi/mezgeb.v1.yaml`), and the
//! conversions at the boundary: amounts as decimal strings, times as
//! RFC 3339, IDs as UUIDs.

use std::collections::BTreeMap;

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use mezgeb_core::{
    Account, Currency, EntryKind, Hold, IdempotencyKey, Money, PostedEntry, Side, Timestamp,
};
use mezgeb_engine::{AdjustmentRequest, EntryRecord, Period};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::problem::ApiError;

/// An amount with its currency: `{"amount": "39.20", "currency": "ETB"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoneyJson {
    pub amount: String,
    pub currency: String,
}

impl MoneyJson {
    /// The amount, refusing an unknown currency or more decimal places than
    /// the currency has. `pointer` names the field in errors.
    pub fn to_money(&self, pointer: &str) -> Result<Money, ApiError> {
        let currency = Currency::from_code(&self.currency).map_err(|_| {
            ApiError::invalid(
                &format!("{pointer}/currency"),
                format!("{:?} is not a supported currency", self.currency),
            )
        })?;
        Money::parse(&self.amount, currency).map_err(|e| {
            ApiError::invalid(
                &format!("{pointer}/amount"),
                format!(
                    "{:?} is not a valid {} amount: {e}",
                    self.amount,
                    currency.code()
                ),
            )
        })
    }
}

impl From<Money> for MoneyJson {
    fn from(money: Money) -> MoneyJson {
        MoneyJson {
            amount: money.to_decimal_string(),
            currency: money.currency().code().to_owned(),
        }
    }
}

/// RFC 3339, in UTC, with microseconds.
pub fn format_time(time: Timestamp) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(time.as_micros()) * 1_000)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// Parses an RFC 3339 time; `pointer` names the field in errors.
pub fn parse_time(text: &str, pointer: &str) -> Result<Timestamp, ApiError> {
    let time = OffsetDateTime::parse(text, &Rfc3339)
        .map_err(|_| ApiError::invalid(pointer, format!("{text:?} is not an RFC 3339 time")))?;
    let micros = time.unix_timestamp_nanos() / 1_000;
    i64::try_from(micros)
        .map(Timestamp::from_micros)
        .map_err(|_| ApiError::invalid(pointer, format!("{text:?} is out of range")))
}

/// The current time, from the service's clock.
pub fn now() -> Timestamp {
    let micros = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000;
    Timestamp::from_micros(i64::try_from(micros).unwrap_or(i64::MAX))
}

/// A JSON request body. Every way the body can be wrong, including too
/// large, is `400 invalid_request`.
pub struct JsonBody<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for JsonBody<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<JsonBody<T>, ApiError> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(match rejection {
                JsonRejection::JsonDataError(e) => ApiError::invalid("", e.body_text()),
                JsonRejection::JsonSyntaxError(e) => ApiError::invalid("", e.body_text()),
                JsonRejection::MissingJsonContentType(_) => ApiError::invalid(
                    "",
                    "The body must be JSON, with Content-Type: application/json.",
                ),
                JsonRejection::BytesRejection(_) => {
                    ApiError::invalid("", "The body is too large or could not be read.")
                }
                other => ApiError::invalid("", other.body_text()),
            }),
        }
    }
}

/// The `Idempotency-Key` header of a write.
pub struct IdemKey(pub IdempotencyKey);

impl<S: Send + Sync> FromRequestParts<S> for IdemKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<IdemKey, ApiError> {
        let value = parts.headers.get("idempotency-key").ok_or_else(|| {
            ApiError::new(
                "idempotency_key_required",
                "This write needs an Idempotency-Key header.",
            )
        })?;
        let text = value.to_str().map_err(|_| {
            ApiError::invalid(
                "/headers/Idempotency-Key",
                "The Idempotency-Key is not text.",
            )
        })?;
        IdempotencyKey::new(text)
            .map(IdemKey)
            .map_err(|e| ApiError::invalid("/headers/Idempotency-Key", e.to_string()))
    }
}

/// Parses a path or query ID.
pub fn parse_id<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, ApiError> {
    text.parse().map_err(|_| ApiError::not_found(what))
}

#[derive(Debug, Serialize)]
pub struct AccountJson {
    pub id: String,
    pub reference: String,
    pub code: u16,
    pub sub_account: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub normal_side: &'static str,
    pub currency: &'static str,
    pub status: &'static str,
    pub allow_negative: bool,
    pub balance: MoneyJson,
    pub debits_total: MoneyJson,
    pub credits_total: MoneyJson,
}

impl From<&Account> for AccountJson {
    fn from(a: &Account) -> AccountJson {
        let currency = a.currency();
        AccountJson {
            id: a.id().to_string(),
            reference: a.reference(),
            code: a.code().value(),
            sub_account: a.sub_account().map(str::to_owned),
            name: a.name().to_owned(),
            kind: a.account_type().as_str(),
            normal_side: a.normal_side().as_str(),
            currency: currency.code(),
            status: a.status().as_str(),
            allow_negative: a.allow_negative(),
            balance: a.balance().into(),
            debits_total: Money::from_minor(a.debits_total(), currency).into(),
            credits_total: Money::from_minor(a.credits_total(), currency).into(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LineJson {
    pub account: String,
    pub side: &'static str,
    pub amount: MoneyJson,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationJson {
    pub reason: String,
    pub requested_by: String,
    pub approved_by: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EntryJson {
    pub id: String,
    pub sequence: u64,
    pub kind: &'static str,
    pub template: Option<String>,
    pub period_id: Option<i64>,
    pub recorded_at: String,
    pub event_time: Option<String>,
    pub lines: Vec<LineJson>,
    pub metadata: BTreeMap<String, String>,
    pub reverses: Option<String>,
    pub refunds: Option<String>,
    pub authorization: Option<AuthorizationJson>,
    pub receipt: Option<serde_json::Value>,
}

fn kind_name(kind: EntryKind) -> &'static str {
    kind.as_str()
}

impl From<&EntryRecord> for EntryJson {
    fn from(record: &EntryRecord) -> EntryJson {
        let e: &PostedEntry = &record.entry;
        EntryJson {
            id: e.id().to_string(),
            sequence: e.sequence(),
            kind: kind_name(e.kind()),
            template: e.metadata().get(mezgeb_ops::TEMPLATE_KEY).cloned(),
            period_id: record.period_id,
            recorded_at: format_time(record.recorded_at),
            event_time: e.event_time().map(format_time),
            lines: e
                .lines()
                .iter()
                .map(|l| LineJson {
                    account: l.account().to_string(),
                    side: l.side().as_str(),
                    amount: l.amount().money().into(),
                })
                .collect(),
            metadata: e.metadata().clone(),
            reverses: e.reverses().map(|r| r.to_string()),
            refunds: e.refunds().map(|r| r.to_string()),
            authorization: e.authorization().map(|a| AuthorizationJson {
                reason: a.reason.as_str().to_owned(),
                requested_by: a.requested_by.as_str().to_owned(),
                approved_by: a.approved_by.as_ref().map(|p| p.as_str().to_owned()),
            }),
            receipt: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HoldJson {
    pub id: String,
    pub account: String,
    pub held_account: String,
    pub amount: MoneyJson,
    pub captured: MoneyJson,
    pub status: &'static str,
    pub expires_at: String,
    pub place_entry: String,
    pub close_entry: Option<String>,
}

impl From<&Hold> for HoldJson {
    fn from(h: &Hold) -> HoldJson {
        let currency = h.amount().currency();
        HoldJson {
            id: h.id().to_string(),
            account: h.account().to_string(),
            held_account: h.held_account().to_string(),
            amount: h.amount().money().into(),
            captured: Money::from_minor(h.captured(), currency).into(),
            status: h.status().as_str(),
            expires_at: format_time(h.expires_at()),
            place_entry: h.place_entry().to_string(),
            close_entry: h.close_entry().map(|e| e.to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PeriodJson {
    pub id: i64,
    pub label: String,
    pub status: &'static str,
    pub opened_at: String,
    pub after_sequence: u64,
    pub last_sequence: Option<u64>,
    pub closing_at: Option<String>,
    pub closed_at: Option<String>,
}

impl From<&Period> for PeriodJson {
    fn from(p: &Period) -> PeriodJson {
        PeriodJson {
            id: p.id,
            label: p.label.clone(),
            status: p.status.as_str(),
            opened_at: format_time(p.opened_at),
            after_sequence: p.after_sequence,
            last_sequence: p.last_sequence,
            closing_at: p.closing_at.map(format_time),
            closed_at: p.closed_at.map(format_time),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AdjustmentJson {
    pub id: String,
    pub status: &'static str,
    pub reason: String,
    pub lines: Vec<LineJson>,
    pub metadata: BTreeMap<String, String>,
    pub requested_by: String,
    pub requested_at: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub entry: Option<String>,
}

impl From<&AdjustmentRequest> for AdjustmentJson {
    fn from(r: &AdjustmentRequest) -> AdjustmentJson {
        AdjustmentJson {
            id: r.id.to_string(),
            status: r.status.as_str(),
            reason: r.reason.as_str().to_owned(),
            lines: r
                .lines
                .iter()
                .map(|l| LineJson {
                    account: l.account.to_string(),
                    side: l.side.as_str(),
                    amount: l.amount.into(),
                })
                .collect(),
            metadata: r.metadata.clone(),
            requested_by: r.requested_by.as_str().to_owned(),
            requested_at: format_time(r.requested_at),
            decided_by: r.decided_by.as_ref().map(|a| a.as_str().to_owned()),
            decided_at: r.decided_at.map(format_time),
            entry: r.entry.map(|e| e.to_string()),
        }
    }
}

/// A side, from its name.
pub fn parse_side(text: &str, pointer: &str) -> Result<Side, ApiError> {
    Side::from_name(text)
        .ok_or_else(|| ApiError::invalid(pointer, format!("{text:?} is not debit or credit")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_strict_decimal_strings() {
        let etb = |amount: &str| MoneyJson {
            amount: amount.into(),
            currency: "ETB".into(),
        };
        assert_eq!(etb("39.20").to_money("/a").map(Money::amount), Ok(3_920));
        assert_eq!(etb("39.2").to_money("/a").map(Money::amount), Ok(3_920));
        assert_eq!(etb("-5").to_money("/a").map(Money::amount), Ok(-500));
        for bad in [
            "39.205",
            "1e3",
            "",
            "abc",
            " 1",
            "+1",
            "1,000",
            "99999999999999999999",
        ] {
            assert_eq!(
                etb(bad).to_money("/a").map_err(|e| e.code()),
                Err("invalid_request"),
                "{bad:?}"
            );
        }
        let unknown = MoneyJson {
            amount: "1".into(),
            currency: "XXX".into(),
        };
        assert_eq!(
            unknown.to_money("/a").map_err(|e| e.code()),
            Err("invalid_request")
        );
        assert_eq!(
            MoneyJson::from(Money::from_minor(3_920, Currency::ETB)),
            etb("39.20")
        );
    }

    #[test]
    fn times_round_trip_with_microseconds() -> Result<(), ApiError> {
        let t = parse_time("2026-11-14T09:30:12.418205Z", "/t")?;
        assert_eq!(format_time(t), "2026-11-14T09:30:12.418205Z");
        assert_eq!(
            parse_time("2026-11-14T12:30:12+03:00", "/t")?,
            parse_time("2026-11-14T09:30:12Z", "/t")?
        );
        assert!(parse_time("yesterday", "/t").is_err());
        Ok(())
    }
}

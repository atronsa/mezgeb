//! An entry and its lines.

use std::collections::BTreeMap;

use mezgeb_core::{EntryKind, PostedEntry, Side};
use mezgeb_engine::EntryRecord;
use serde::Serialize;

use super::*;
use crate::problem::ApiError;

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
    pub receipt: Option<ReceiptJson>,
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
            receipt: record.receipt.as_ref().map(ReceiptJson::from),
        }
    }
}

/// A side, from its name.
pub fn parse_side(text: &str, pointer: &str) -> Result<Side, ApiError> {
    Side::from_name(text)
        .ok_or_else(|| ApiError::invalid(pointer, format!("{text:?} is not debit or credit")))
}

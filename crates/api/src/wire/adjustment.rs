//! An adjustment request.

use std::collections::BTreeMap;

use mezgeb_engine::AdjustmentRequest;
use serde::Serialize;

use super::*;

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

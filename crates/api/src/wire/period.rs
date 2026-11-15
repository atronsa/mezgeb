//! An accounting period.

use mezgeb_engine::Period;
use serde::Serialize;

use super::*;

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

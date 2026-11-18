//! Reconciliation runs and breaks (step 2.15, ADR-0020).

use mezgeb_core::{Currency, Money};
use mezgeb_engine::recon::{CacheResult, ReconBreak, ReconRun};
use serde::Serialize;

use super::{MoneyJson, format_time};

/// One run, as recorded.
#[derive(Debug, Serialize)]
pub struct ReconRunJson {
    pub id: String,
    pub kind: &'static str,
    pub account: Option<String>,
    pub client: Option<String>,
    pub source: String,
    pub as_of: String,
    pub items: i32,
    pub matched: i32,
    pub pending: i32,
    pub breaks_new: i32,
    pub breaks_cleared: i32,
    pub breaks_open: i32,
    pub runtime_ms: i64,
    pub recorded_at: String,
}

impl From<&ReconRun> for ReconRunJson {
    fn from(r: &ReconRun) -> ReconRunJson {
        ReconRunJson {
            id: r.id.to_string(),
            kind: r.kind.as_str(),
            account: r.account.map(|a| a.to_string()),
            client: r.client.map(|c| c.to_string()),
            source: r.source.clone(),
            as_of: format_time(r.as_of),
            items: r.items,
            matched: r.matched,
            pending: r.pending,
            breaks_new: r.breaks_new,
            breaks_cleared: r.breaks_cleared,
            breaks_open: r.breaks_open,
            runtime_ms: r.runtime_micros / 1000,
            recorded_at: format_time(r.recorded_at),
        }
    }
}

/// A ledger line: its entry and line number.
#[derive(Debug, Serialize)]
pub struct LineRefJson {
    pub entry: String,
    pub line: i16,
}

/// One break.
#[derive(Debug, Serialize)]
pub struct ReconBreakJson {
    pub id: String,
    pub run: String,
    pub kind: &'static str,
    pub status: &'static str,
    pub subject: String,
    pub account: Option<String>,
    pub client: Option<String>,
    pub statement_line: Option<String>,
    pub ledger_line: Option<LineRefJson>,
    pub device: Option<String>,
    pub voucher: Option<String>,
    /// What the other record says, what the ledger says, and the
    /// difference (other minus ledger).
    pub other: Option<MoneyJson>,
    pub ledger: Option<MoneyJson>,
    pub difference: Option<MoneyJson>,
    pub detail: String,
    pub found_at: String,
    pub cleared_by_run: Option<String>,
    pub resolution: Option<LineRefJson>,
    pub note: Option<String>,
    pub closed_by: Option<String>,
    pub closed_at: Option<String>,
}

impl From<&ReconBreak> for ReconBreakJson {
    fn from(b: &ReconBreak) -> ReconBreakJson {
        let money = |minor: Option<i64>| -> Option<MoneyJson> {
            Some(Money::from_minor(minor?, b.currency?).into())
        };
        let line = |line: Option<(uuid::Uuid, i16)>| {
            line.map(|(entry, line)| LineRefJson {
                entry: entry.to_string(),
                line,
            })
        };
        ReconBreakJson {
            id: b.id.to_string(),
            run: b.run_id.to_string(),
            kind: b.kind.as_str(),
            status: b.status.as_str(),
            subject: b.subject.clone(),
            account: b.account.map(|a| a.to_string()),
            client: b.client.map(|c| c.to_string()),
            statement_line: b.item.map(|i| i.to_string()),
            ledger_line: line(b.line),
            device: b.device.map(|d| d.to_string()),
            voucher: b.voucher.map(|v| v.to_string()),
            other: money(b.other_minor),
            ledger: money(b.ledger_minor),
            difference: money(b.difference_minor),
            detail: b.detail.clone(),
            found_at: format_time(b.found_at),
            cleared_by_run: b.cleared_by_run.map(|r| r.to_string()),
            resolution: line(b.resolution),
            note: b.resolution_note.clone(),
            closed_by: b.closed_by.map(|c| c.to_string()),
            closed_at: b.closed_at.map(format_time),
        }
    }
}

/// One cached balance compared with the ledger.
#[derive(Debug, Serialize)]
pub struct CacheResultJson {
    pub account: String,
    pub cached: MoneyJson,
    pub ledger: MoneyJson,
    pub as_of_sequence: i64,
    pub agrees: bool,
}

impl CacheResultJson {
    pub fn new(r: &CacheResult, currency: Currency) -> CacheResultJson {
        CacheResultJson {
            account: r.account.to_string(),
            cached: Money::from_minor(r.cached_minor, currency).into(),
            ledger: Money::from_minor(r.ledger_minor, currency).into(),
            as_of_sequence: r.as_of_sequence,
            agrees: r.agrees,
        }
    }
}

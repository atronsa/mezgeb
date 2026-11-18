//! Refusals of callers who have not authenticated, audited in summary
//! (ADR-0021).
//!
//! Each such refusal (`401`, a refused token request, an unknown route, a
//! `429` before authentication) is its own audit event only if it is the
//! first for its actor in the minute, and only while fewer than
//! [`EVENTS_PER_MINUTE`] have been written in that minute. The others are
//! counted by outcome, and [`write_summary`] records the counts as one
//! `refusals.suppressed` event, once a minute and at shutdown. The audit
//! chain therefore grows by a bounded number of events a minute, however
//! many requests an attacker sends, and every refusal is still counted in it.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::AppState;

/// The most refusals of unauthenticated callers written as events of their
/// own in a minute.
pub const EVENTS_PER_MINUTE: usize = 60;

/// How often the summary is written.
pub const WINDOW: Duration = Duration::from_secs(60);

/// The action of a summary event.
pub const SUMMARY_ACTION: &str = "refusals.suppressed";

#[derive(Debug, Default)]
struct Window {
    started: Option<Instant>,
    written: usize,
    /// The actors written this minute; at most [`EVENTS_PER_MINUTE`].
    seen: HashSet<String>,
    /// Refusals counted, not written, since the last summary, by outcome.
    suppressed: BTreeMap<String, u64>,
}

/// Which refusals become audit events of their own.
#[derive(Debug, Default)]
pub struct Refusals {
    window: Mutex<Window>,
}

impl Refusals {
    /// Whether a refusal of `actor`, with `outcome`, at `now`, is written as
    /// its own audit event. If not, it is counted for the summary.
    pub fn admit_at(&self, actor: &str, outcome: &str, now: Instant) -> bool {
        let Ok(mut window) = self.window.lock() else {
            return true;
        };
        if window
            .started
            .is_none_or(|s| now.saturating_duration_since(s) >= WINDOW)
        {
            window.started = Some(now);
            window.written = 0;
            window.seen.clear();
        }
        if window.written < EVENTS_PER_MINUTE && window.seen.insert(actor.to_owned()) {
            window.written += 1;
            true
        } else {
            *window.suppressed.entry(outcome.to_owned()).or_default() += 1;
            false
        }
    }

    pub fn admit(&self, actor: &str, outcome: &str) -> bool {
        self.admit_at(actor, outcome, Instant::now())
    }

    /// The refusals counted since the last call, by outcome.
    pub fn take_suppressed(&self) -> BTreeMap<String, u64> {
        self.window
            .lock()
            .map(|mut w| std::mem::take(&mut w.suppressed))
            .unwrap_or_default()
    }
}

/// Records the refusals counted since the last summary as one audit event,
/// if there were any.
pub async fn write_summary(state: &AppState) {
    let counts = state.refusals.take_suppressed();
    if counts.is_empty() {
        return;
    }
    let total: u64 = counts.values().sum();
    let details = serde_json::json!({"total": total, "by_outcome": counts});
    let event = mezgeb_engine::NewAuditEvent {
        client: None,
        actor: None,
        action: SUMMARY_ACTION.to_owned(),
        target: None,
        request_id: None,
        outcome: "suppressed".to_owned(),
        details: details.to_string(),
    };
    let written = match state.pool.acquire().await {
        Ok(mut conn) => mezgeb_engine::append_audit(&mut conn, &event, crate::wire::now())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match written {
        Ok(()) => tracing::warn!(total, "refusals of unauthenticated callers summarised"),
        Err(cause) => tracing::error!(%cause, total, "a refusals summary was not recorded"),
    }
}

/// Writes the summary every [`WINDOW`], until the task is aborted.
pub async fn summarise(state: AppState) {
    let mut every = tokio::time::interval(WINDOW);
    every.tick().await;
    loop {
        every.tick().await;
        write_summary(&state).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_refusal_of_each_actor_in_a_minute_is_written() {
        let refusals = Refusals::default();
        let start = Instant::now();
        assert!(refusals.admit_at("credential:a", "unauthenticated", start));
        assert!(!refusals.admit_at("credential:a", "unauthenticated", start));
        assert!(!refusals.admit_at("credential:a", "rate_limited", start));
        assert!(refusals.admit_at("anonymous", "unauthenticated", start));
        // A minute later, the same actor is written again.
        assert!(refusals.admit_at("credential:a", "unauthenticated", start + WINDOW));
        let counts = refusals.take_suppressed();
        assert_eq!(counts.get("unauthenticated"), Some(&1));
        assert_eq!(counts.get("rate_limited"), Some(&1));
        assert!(refusals.take_suppressed().is_empty(), "taken once");
    }

    #[test]
    fn no_more_than_the_limit_are_written_in_a_minute_whatever_the_actors() {
        let refusals = Refusals::default();
        let start = Instant::now();
        let written = (0..10_000)
            .filter(|n| refusals.admit_at(&format!("credential:{n}"), "unauthenticated", start))
            .count();
        assert_eq!(written, EVENTS_PER_MINUTE);
        assert_eq!(
            refusals.take_suppressed().get("unauthenticated"),
            Some(&(10_000 - EVENTS_PER_MINUTE as u64))
        );
    }
}

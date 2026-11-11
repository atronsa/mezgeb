//! Points in time, passed into the core and never read from a clock inside
//! it, so the core stays deterministic.

use std::fmt;

/// Microseconds since the Unix epoch (1970-01-01T00:00:00Z), the precision
/// PostgreSQL stores.
///
/// Two times appear on an entry (spec §3.6):
/// - `recorded_time`: assigned by the ledger when it posts. Authoritative and
///   strictly increasing (INV-12).
/// - `event_time`: claimed by the client or device, for example when an
///   offline payment really happened. Informational only, never used for
///   ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_micros(micros: i64) -> Timestamp {
        Timestamp(micros)
    }

    pub const fn as_micros(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}µs", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_orders() {
        let t = Timestamp::from_micros(1_762_819_200_000_000);
        assert_eq!(t.as_micros(), 1_762_819_200_000_000);
        assert!(Timestamp::from_micros(1) < Timestamp::from_micros(2));
        assert_eq!(Timestamp::from_micros(5).to_string(), "5µs");
    }
}

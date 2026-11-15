//! Rate limits (ADR-0017, TM-03): a token bucket per client, and one per
//! credential at the token endpoint.
//!
//! A bucket holds up to `burst` requests and refills at `rate` per second.
//! A request that finds it empty is refused with `429` and `Retry-After`;
//! with a rate of at least one per second, one second is always enough for
//! the next request.
//!
//! The buckets live in this process: several instances of the service each
//! allow the full rate (a shared limiter comes with horizontal scaling,
//! step 3.03). The map is bounded: when it grows past [`MAX_BUCKETS`], the
//! buckets that are full again (idle) are dropped, since a full bucket is
//! the same as no bucket.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::Instant;

use mezgeb_core::ClientId;
use uuid::Uuid;

/// The most buckets kept before idle ones are dropped.
pub const MAX_BUCKETS: usize = 10_000;

/// The token endpoint's limit per credential: each attempt costs an
/// Argon2 hash, and a secret should not be guessable at speed.
pub const TOKEN_RATE: u32 = 2;
pub const TOKEN_BURST: u32 = 10;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Buckets by key.
#[derive(Debug)]
pub struct Buckets<K> {
    buckets: Mutex<HashMap<K, Bucket>>,
}

impl<K> Default for Buckets<K> {
    fn default() -> Self {
        Buckets {
            buckets: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash + Copy> Buckets<K> {
    /// Takes one request from `key`'s bucket, at time `now`; `false` if it
    /// is empty.
    pub fn take_at(&self, key: K, rate: u32, burst: u32, now: Instant) -> bool {
        let (rate, burst) = (f64::from(rate.max(1)), f64::from(burst.max(1)));
        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned lock means a panic elsewhere; do not turn it into
            // an outage.
            return true;
        };
        if buckets.len() >= MAX_BUCKETS && !buckets.contains_key(&key) {
            buckets.retain(|_, b| {
                b.tokens + now.saturating_duration_since(b.at).as_secs_f64() * rate < burst
            });
        }
        let bucket = buckets.entry(key).or_insert(Bucket {
            tokens: burst,
            at: now,
        });
        let refill = now.saturating_duration_since(bucket.at).as_secs_f64() * rate;
        bucket.tokens = (bucket.tokens + refill).min(burst);
        bucket.at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn take(&self, key: K, rate: u32, burst: u32) -> bool {
        self.take_at(key, rate, burst, Instant::now())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.buckets.lock().map(|b| b.len()).unwrap_or_default()
    }
}

/// Every limit the service keeps.
#[derive(Debug, Default)]
pub struct Limits {
    /// Per client, at its `rate_per_second` and `burst`.
    pub clients: Buckets<ClientId>,
    /// Per credential, at the token endpoint.
    pub logins: Buckets<Uuid>,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_bucket_allows_its_burst_then_its_rate() {
        let buckets = Buckets::default();
        let start = Instant::now();
        for _ in 0..3 {
            assert!(buckets.take_at(1u8, 2, 3, start));
        }
        assert!(!buckets.take_at(1u8, 2, 3, start), "the burst is spent");
        // At two per second, one more after half a second.
        let half = start + Duration::from_millis(500);
        assert!(buckets.take_at(1u8, 2, 3, half));
        assert!(!buckets.take_at(1u8, 2, 3, half));
        // Another key has its own bucket.
        assert!(buckets.take_at(2u8, 2, 3, half));
        // A long pause refills to the burst, not beyond.
        let later = start + Duration::from_secs(60);
        for _ in 0..3 {
            assert!(buckets.take_at(1u8, 2, 3, later));
        }
        assert!(!buckets.take_at(1u8, 2, 3, later));
    }

    #[test]
    fn idle_buckets_are_dropped_when_there_are_too_many() {
        let buckets = Buckets::default();
        let start = Instant::now();
        for key in 0..MAX_BUCKETS {
            assert!(buckets.take_at(key, 1, 1, start));
        }
        assert_eq!(buckets.len(), MAX_BUCKETS);
        // A second later every bucket is full again: all are dropped to
        // make room.
        assert!(buckets.take_at(MAX_BUCKETS, 1, 1, start + Duration::from_secs(2)));
        assert_eq!(buckets.len(), 1);
    }
}

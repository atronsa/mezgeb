//! Rate limits (ADR-0017, ADR-0021, TM-03): a token bucket per client, one
//! per credential at the token endpoint, one for every failed login in the
//! service, and, if the operator turns it on, one per source address for
//! failed logins and refused tokens.
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
use std::net::IpAddr;
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

/// Failed logins in the whole service (ADR-0021): each costs an Argon2
/// hash, and a caller who invents a new credential ID each time is not
/// slowed by the limit per credential. While these are spent, the token
/// endpoint refuses before looking anything up.
pub const FAILED_LOGIN_RATE: u32 = 20;
pub const FAILED_LOGIN_BURST: u32 = 100;

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
        let level = |b: &Bucket| {
            (b.tokens + now.saturating_duration_since(b.at).as_secs_f64() * rate).min(burst)
        };
        if buckets.len() >= MAX_BUCKETS && !buckets.contains_key(&key) {
            buckets.retain(|_, b| level(b) < burst);
        }
        let bucket = buckets.entry(key).or_insert(Bucket {
            tokens: burst,
            at: now,
        });
        bucket.tokens = level(bucket);
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

    /// Gives back one request taken from `key`'s bucket, for work that
    /// turned out not to count against it.
    pub fn refund(&self, key: K, burst: u32) {
        if let Ok(mut buckets) = self.buckets.lock()
            && let Some(bucket) = buckets.get_mut(&key)
        {
            bucket.tokens = (bucket.tokens + 1.0).min(f64::from(burst.max(1)));
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.buckets.lock().map(|b| b.len()).unwrap_or_default()
    }
}

/// Where a caller's address comes from, for the limit per address.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AddressSource {
    /// The TCP peer: right when nothing stands in front of the service.
    #[default]
    Peer,
    /// The last entry of `X-Forwarded-For`, the one the operator's own
    /// proxy added; the peer if there is none. Only for a service that is
    /// reachable through that proxy alone.
    ForwardedFor,
}

/// The limit per source address on failed logins and refused tokens
/// (ADR-0021): off unless the operator sets a rate.
#[derive(Debug, Clone, Copy, Default)]
pub struct AddressLimit {
    /// Failures per second per address; `0` turns the limit off.
    pub per_second: u32,
    pub source: AddressSource,
}

impl AddressLimit {
    /// The burst: ten seconds' worth.
    pub fn burst(&self) -> u32 {
        self.per_second.saturating_mul(10)
    }
}

/// Every limit the service keeps.
#[derive(Debug, Default)]
pub struct Limits {
    /// Per client, at its `rate_per_second` and `burst`.
    pub clients: Buckets<ClientId>,
    /// Per credential, at the token endpoint.
    pub logins: Buckets<Uuid>,
    /// Failed logins in the whole service.
    pub failed_logins: Buckets<()>,
    /// Failures per source address, when [`Limits::address`] is on.
    pub addresses: Buckets<IpAddr>,
    pub address: AddressLimit,
}

impl Limits {
    /// Limits with the per-address limit set as given.
    pub fn with_address_limit(address: AddressLimit) -> Limits {
        Limits {
            address,
            ..Limits::default()
        }
    }

    // Each attempt reserves a failure before the work, and gives it back if
    // it succeeds: checking first and counting after would let a burst of
    // concurrent attempts all pass the check before the first one failed.

    /// Reserves a failure for an attempt from `address`: `false` once its
    /// failures are spent, if the limit per address is on.
    pub fn reserve_address(&self, address: Option<IpAddr>) -> bool {
        match (self.address.per_second, address) {
            (0, _) | (_, None) => true,
            (rate, Some(ip)) => self.addresses.take(ip, rate, self.address.burst()),
        }
    }

    /// Gives back the failure reserved for `address`: the attempt succeeded.
    pub fn release_address(&self, address: Option<IpAddr>) {
        if let (1.., Some(ip)) = (self.address.per_second, address) {
            self.addresses.refund(ip, self.address.burst());
        }
    }

    /// Reserves a failed login before a secret is checked, in the whole
    /// service and for `address`: `false` while either is spent.
    pub fn reserve_login(&self, address: Option<IpAddr>) -> bool {
        if !self
            .failed_logins
            .take((), FAILED_LOGIN_RATE, FAILED_LOGIN_BURST)
        {
            return false;
        }
        if !self.reserve_address(address) {
            self.failed_logins.refund((), FAILED_LOGIN_BURST);
            return false;
        }
        true
    }

    /// Gives back the failed login reserved: the secret was right.
    pub fn release_login(&self, address: Option<IpAddr>) {
        self.failed_logins.refund((), FAILED_LOGIN_BURST);
        self.release_address(address);
    }
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
    fn a_refund_gives_back_one_up_to_the_burst() {
        let buckets = Buckets::default();
        let start = Instant::now();
        buckets.refund(1u8, 2);
        assert_eq!(buckets.len(), 0, "nothing to give back to");
        assert!(buckets.take_at(1u8, 1, 2, start));
        assert!(buckets.take_at(1u8, 1, 2, start));
        assert!(!buckets.take_at(1u8, 1, 2, start));
        buckets.refund(1u8, 2);
        assert!(buckets.take_at(1u8, 1, 2, start));
        buckets.refund(1u8, 2);
        buckets.refund(1u8, 2);
        buckets.refund(1u8, 2);
        assert!(buckets.take_at(1u8, 1, 2, start));
        assert!(buckets.take_at(1u8, 1, 2, start));
        assert!(!buckets.take_at(1u8, 1, 2, start), "never beyond the burst");
    }

    #[test]
    fn failed_logins_are_limited_in_the_whole_service() {
        let limits = Limits::default();
        // Successful logins give their reservation back.
        for _ in 0..1_000 {
            assert!(limits.reserve_login(None));
            limits.release_login(None);
        }
        for _ in 0..FAILED_LOGIN_BURST {
            assert!(limits.reserve_login(None));
        }
        assert!(!limits.reserve_login(None), "the burst is spent");
    }

    #[test]
    fn the_address_limit_is_off_unless_set() {
        let ip: IpAddr = [192, 0, 2, 7].into();
        let off = Limits::default();
        for _ in 0..1_000 {
            assert!(off.reserve_address(Some(ip)));
        }

        let on = Limits::with_address_limit(AddressLimit {
            per_second: 1,
            source: AddressSource::Peer,
        });
        for _ in 0..10 {
            assert!(on.reserve_address(Some(ip)));
        }
        assert!(!on.reserve_address(Some(ip)), "ten failures, the burst");
        on.release_address(Some(ip));
        assert!(on.reserve_address(Some(ip)), "one given back");
        assert!(
            on.reserve_address(Some([192, 0, 2, 8].into())),
            "another address"
        );
        assert!(on.reserve_address(None), "no address known");
        // A login refused by the address does not spend the service's.
        let before = (0..FAILED_LOGIN_BURST)
            .take_while(|_| on.reserve_login(Some(ip)))
            .count();
        assert_eq!(before, 0);
        assert!(on.reserve_login(None));
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

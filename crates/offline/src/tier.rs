//! Trust tiers and their limits (protocol §6, ADR-0019).
//!
//! | | Tier A | Tier B | Tier C |
//! |---|---|---|---|
//! | Device | Smart card, secure element | Phone, attested StrongBox/TEE key | Phone, software key |
//! | Per voucher (max) | 2,000 ETB | 1,000 ETB | 200 ETB |
//! | Voucher lifetime | 7 days | 3 days | 1 day |
//!
//! Every tier has 7 days of grace, and a customer's outstanding escrow
//! across all devices is at most 5,000 ETB. A client may lower a limit or
//! disable a tier, never raise a limit above these maxima.

use std::fmt;
use std::str::FromStr;

/// A microsecond.
const DAY_MICROS: i64 = 86_400 * 1_000_000;

/// Grace after a voucher expires, during which its receipts still settle
/// from escrow.
pub const GRACE_MICROS: i64 = 7 * DAY_MICROS;

/// The most outstanding escrow one customer may have across all their
/// devices, in ETB santim.
pub const CUSTOMER_CAP_MINOR: i64 = 500_000;

/// How long a device certificate is valid: one year.
pub const CERTIFICATE_MICROS: i64 = 365 * DAY_MICROS;

/// How far a device's hardware can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// A smart card with a secure element.
    A,
    /// A phone whose key is in attested StrongBox or TEE.
    B,
    /// A phone with a software key.
    C,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::A, Tier::B, Tier::C];

    /// Its code in signed messages: 1, 2 or 3.
    pub const fn code(self) -> u8 {
        match self {
            Tier::A => 1,
            Tier::B => 2,
            Tier::C => 3,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Tier::A => "A",
            Tier::B => "B",
            Tier::C => "C",
        }
    }

    /// The protocol's maximum limits for this tier.
    pub const fn maximum(self) -> TierLimits {
        match self {
            Tier::A => TierLimits {
                max_voucher_minor: 200_000,
                lifetime_micros: 7 * DAY_MICROS,
            },
            Tier::B => TierLimits {
                max_voucher_minor: 100_000,
                lifetime_micros: 3 * DAY_MICROS,
            },
            Tier::C => TierLimits {
                max_voucher_minor: 20_000,
                lifetime_micros: DAY_MICROS,
            },
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Not `A`, `B` or `C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTier(pub String);

impl fmt::Display for UnknownTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown tier {:?}", self.0)
    }
}

impl std::error::Error for UnknownTier {}

impl FromStr for Tier {
    type Err = UnknownTier;

    fn from_str(s: &str) -> Result<Tier, UnknownTier> {
        match s {
            "A" => Ok(Tier::A),
            "B" => Ok(Tier::B),
            "C" => Ok(Tier::C),
            other => Err(UnknownTier(other.to_owned())),
        }
    }
}

/// One tier's limits: the largest voucher, in ETB santim, and how long a
/// voucher lives before its grace starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierLimits {
    pub max_voucher_minor: i64,
    pub lifetime_micros: i64,
}

impl TierLimits {
    /// Whether these limits are within `tier`'s maxima: a client may lower
    /// them, never raise them.
    pub fn within(self, tier: Tier) -> bool {
        let max = tier.maximum();
        self.max_voucher_minor > 0
            && self.max_voucher_minor <= max.max_voucher_minor
            && self.lifetime_micros > 0
            && self.lifetime_micros <= max.lifetime_micros
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_maxima_are_the_protocols() {
        assert_eq!(Tier::A.maximum().max_voucher_minor, 200_000);
        assert_eq!(Tier::B.maximum().max_voucher_minor, 100_000);
        assert_eq!(Tier::C.maximum().max_voucher_minor, 20_000);
        assert!(Tier::A.maximum().lifetime_micros > Tier::B.maximum().lifetime_micros);
        for tier in Tier::ALL {
            assert!(tier.maximum().within(tier));
            assert_eq!(tier.as_str().parse(), Ok(tier));
        }
    }

    #[test]
    fn limits_can_be_lowered_not_raised() {
        let mut lower = Tier::B.maximum();
        lower.max_voucher_minor = 50_000;
        assert!(lower.within(Tier::B));
        assert!(!Tier::A.maximum().within(Tier::B));
        lower.max_voucher_minor = 0;
        assert!(!lower.within(Tier::B));
    }
}

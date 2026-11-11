//! Currencies and their minor units (ISO 4217, ADR-0005).
//!
//! A [`Currency`] can only be obtained from the built-in table, so every
//! currency in the ledger has a known code, numeric code and exponent.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An ISO 4217 currency with its minor-unit exponent.
///
/// The exponent is the number of decimal places of the minor unit: 2 for ETB
/// (1 birr = 100 santim), 0 for JPY, 3 for KWD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Currency {
    code: &'static str,
    numeric: u16,
    exponent: u8,
}

/// The error returned when a currency code is not in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCurrency(pub String);

impl fmt::Display for UnknownCurrency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown currency code {:?}", self.0)
    }
}

impl std::error::Error for UnknownCurrency {}

macro_rules! currencies {
    ($($name:ident = ($code:literal, $numeric:literal, $exponent:literal), $doc:literal;)+) => {
        impl Currency {
            $(
                #[doc = $doc]
                pub const $name: Currency = Currency { code: $code, numeric: $numeric, exponent: $exponent };
            )+

            /// Every supported currency. ETB is first.
            pub const ALL: &'static [Currency] = &[$(Currency::$name),+];
        }
    };
}

currencies! {
    ETB = ("ETB", 230, 2), "Ethiopian birr (100 santim).";
    USD = ("USD", 840, 2), "US dollar.";
    EUR = ("EUR", 978, 2), "Euro.";
    GBP = ("GBP", 826, 2), "Pound sterling.";
    KES = ("KES", 404, 2), "Kenyan shilling.";
    TZS = ("TZS", 834, 2), "Tanzanian shilling.";
    UGX = ("UGX", 800, 0), "Ugandan shilling (no minor unit).";
    NGN = ("NGN", 566, 2), "Nigerian naira.";
    ZAR = ("ZAR", 710, 2), "South African rand.";
    JPY = ("JPY", 392, 0), "Japanese yen (no minor unit).";
    KWD = ("KWD", 414, 3), "Kuwaiti dinar (1,000 fils).";
}

impl Currency {
    /// Looks up a currency by its alphabetic code, such as `"ETB"`.
    /// The code must be exactly three uppercase letters.
    pub fn from_code(code: &str) -> Result<Currency, UnknownCurrency> {
        Currency::ALL
            .iter()
            .find(|c| c.code == code)
            .copied()
            .ok_or_else(|| UnknownCurrency(code.to_owned()))
    }

    /// The alphabetic code, such as `"ETB"`.
    pub const fn code(self) -> &'static str {
        self.code
    }

    /// The numeric code, such as `230` for ETB.
    pub const fn numeric(self) -> u16 {
        self.numeric
    }

    /// The number of decimal places of the minor unit.
    pub const fn exponent(self) -> u8 {
        self.exponent
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl Serialize for Currency {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code)
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Currency::from_code(&code).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etb_is_first_with_two_decimals() {
        assert_eq!(Currency::ALL[0], Currency::ETB);
        assert_eq!(Currency::ETB.code(), "ETB");
        assert_eq!(Currency::ETB.numeric(), 230);
        assert_eq!(Currency::ETB.exponent(), 2);
    }

    #[test]
    fn looks_up_known_codes() {
        assert_eq!(Currency::from_code("ETB"), Ok(Currency::ETB));
        assert_eq!(Currency::from_code("JPY"), Ok(Currency::JPY));
    }

    #[test]
    fn rejects_unknown_or_badly_cased_codes() {
        for code in ["", "etb", "XXX", "ETBB", " ETB"] {
            assert_eq!(
                Currency::from_code(code),
                Err(UnknownCurrency(code.to_owned()))
            );
        }
        assert_eq!(
            UnknownCurrency("XXX".into()).to_string(),
            "unknown currency code \"XXX\""
        );
    }

    #[test]
    fn table_has_unique_codes_and_numbers() {
        for (i, a) in Currency::ALL.iter().enumerate() {
            for b in &Currency::ALL[i + 1..] {
                assert_ne!(a.code(), b.code());
                assert_ne!(a.numeric(), b.numeric());
            }
            assert_eq!(a.code().len(), 3);
            assert!(a.code().bytes().all(|b| b.is_ascii_uppercase()));
            assert!(a.exponent() <= 3);
        }
    }

    #[test]
    fn serializes_as_its_code() -> Result<(), serde_json::Error> {
        assert_eq!(serde_json::to_string(&Currency::ETB)?, "\"ETB\"");
        assert_eq!(serde_json::from_str::<Currency>("\"KWD\"")?, Currency::KWD);
        assert!(serde_json::from_str::<Currency>("\"XXX\"").is_err());
        assert!(serde_json::from_str::<Currency>("230").is_err());
        assert_eq!(Currency::USD.to_string(), "USD");
        Ok(())
    }
}

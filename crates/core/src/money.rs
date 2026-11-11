//! Money as an `i64` count of minor units (ADR-0005).
//!
//! There are no floats and no rounding anywhere in this module. Every
//! arithmetic operation is checked: overflow and mixed currencies are
//! errors, never a wrap or a silent conversion. There is deliberately no
//! `impl Add`, so `a + b` on money does not compile.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::currency::Currency;

/// Longest decimal string accepted by [`Money::parse`]. Longer input is
/// rejected before any arithmetic. The longest valid `i64` amount is 21
/// characters (`-9223372036854775808`), plus a decimal point.
pub const MAX_DECIMAL_LEN: usize = 32;

/// An amount of money in one currency, stored as a count of minor units
/// (santim for ETB). It may be negative, for example a balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Money {
    amount: i64,
    currency: Currency,
}

/// An amount of money that is strictly greater than zero, as every journal
/// line amount must be (INV-1). The side of the line, not the sign, gives
/// the direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PositiveAmount(Money);

/// Errors from arithmetic on money.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    /// The two amounts are in different currencies.
    CurrencyMismatch { left: Currency, right: Currency },
    /// The result does not fit in an `i64`.
    Overflow,
    /// The amount must be greater than zero.
    NotPositive,
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoneyError::CurrencyMismatch { left, right } => {
                write!(f, "currency mismatch: {left} and {right}")
            }
            MoneyError::Overflow => f.write_str("amount overflow"),
            MoneyError::NotPositive => f.write_str("amount must be greater than zero"),
        }
    }
}

impl std::error::Error for MoneyError {}

/// Errors from parsing a decimal string such as `"39.20"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMoneyError {
    /// The string is empty.
    Empty,
    /// The string is longer than [`MAX_DECIMAL_LEN`].
    TooLong,
    /// A character other than a digit, `.` or a leading `-`.
    InvalidCharacter,
    /// Misplaced or missing parts, such as `".5"`, `"5."` or `"1.2.3"`.
    InvalidFormat,
    /// The integer part has a leading zero, such as `"007"`.
    LeadingZero,
    /// More decimal places than the currency allows. Never rounded.
    TooManyDecimals { max: u8 },
    /// `"-0"` or `"-0.00"`.
    NegativeZero,
    /// The value does not fit in an `i64` count of minor units.
    Overflow,
    /// The amount must be greater than zero.
    NotPositive,
}

impl fmt::Display for ParseMoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseMoneyError::Empty => f.write_str("amount is empty"),
            ParseMoneyError::TooLong => f.write_str("amount is too long"),
            ParseMoneyError::InvalidCharacter => f.write_str("amount has an invalid character"),
            ParseMoneyError::InvalidFormat => f.write_str("amount is not a valid decimal"),
            ParseMoneyError::LeadingZero => f.write_str("amount has a leading zero"),
            ParseMoneyError::TooManyDecimals { max } => {
                write!(f, "amount has more than {max} decimal places")
            }
            ParseMoneyError::NegativeZero => f.write_str("amount is negative zero"),
            ParseMoneyError::Overflow => f.write_str("amount is too large"),
            ParseMoneyError::NotPositive => f.write_str("amount must be greater than zero"),
        }
    }
}

impl std::error::Error for ParseMoneyError {}

impl Money {
    /// Creates money from a count of minor units: `Money::from_minor(3920,
    /// Currency::ETB)` is 39.20 ETB.
    pub const fn from_minor(amount: i64, currency: Currency) -> Money {
        Money { amount, currency }
    }

    /// Zero in the given currency.
    pub const fn zero(currency: Currency) -> Money {
        Money::from_minor(0, currency)
    }

    /// The count of minor units.
    pub const fn amount(self) -> i64 {
        self.amount
    }

    /// The currency.
    pub const fn currency(self) -> Currency {
        self.currency
    }

    pub const fn is_zero(self) -> bool {
        self.amount == 0
    }

    pub const fn is_positive(self) -> bool {
        self.amount > 0
    }

    pub const fn is_negative(self) -> bool {
        self.amount < 0
    }

    /// Adds two amounts in the same currency.
    pub fn checked_add(self, other: Money) -> Result<Money, MoneyError> {
        self.same_currency(other)?;
        self.with_amount(self.amount.checked_add(other.amount))
    }

    /// Subtracts an amount in the same currency.
    pub fn checked_sub(self, other: Money) -> Result<Money, MoneyError> {
        self.same_currency(other)?;
        self.with_amount(self.amount.checked_sub(other.amount))
    }

    /// The same amount with the opposite sign. Fails only for `i64::MIN`.
    pub fn checked_neg(self) -> Result<Money, MoneyError> {
        self.with_amount(self.amount.checked_neg())
    }

    fn same_currency(self, other: Money) -> Result<(), MoneyError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            })
        }
    }

    fn with_amount(self, amount: Option<i64>) -> Result<Money, MoneyError> {
        amount
            .map(|amount| Money::from_minor(amount, self.currency))
            .ok_or(MoneyError::Overflow)
    }

    /// Parses a decimal string in the given currency: `"39.20"` in ETB is
    /// 3,920 santim.
    ///
    /// The rules are exact, so the same amount always has one accepted
    /// meaning:
    /// - digits, an optional leading `-`, and at most one `.`;
    /// - at least one digit before and after the `.`;
    /// - no leading zeros (`"0.50"` is fine, `"00.50"` is not);
    /// - at most as many decimal places as the currency has (`"0.001"` ETB
    ///   is rejected, never rounded); fewer are allowed (`"39.2"`);
    /// - no `"-0"`, no `+`, no spaces, no exponents.
    pub fn parse(s: &str, currency: Currency) -> Result<Money, ParseMoneyError> {
        if s.is_empty() {
            return Err(ParseMoneyError::Empty);
        }
        if s.len() > MAX_DECIMAL_LEN {
            return Err(ParseMoneyError::TooLong);
        }
        let (negative, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        if !body.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return Err(ParseMoneyError::InvalidCharacter);
        }
        let (int_part, frac_part) = match body.split_once('.') {
            Some((int_part, frac_part)) => (int_part, Some(frac_part)),
            None => (body, None),
        };
        if int_part.is_empty() {
            return Err(ParseMoneyError::InvalidFormat);
        }
        if int_part.len() > 1 && int_part.starts_with('0') {
            return Err(ParseMoneyError::LeadingZero);
        }
        let frac_digits = frac_part.unwrap_or("");
        if frac_part.is_some() && (frac_digits.is_empty() || frac_digits.contains('.')) {
            return Err(ParseMoneyError::InvalidFormat);
        }
        let exponent = currency.exponent();
        if frac_digits.len() > usize::from(exponent) {
            return Err(ParseMoneyError::TooManyDecimals { max: exponent });
        }

        // At most MAX_DECIMAL_LEN digits plus `exponent` padding zeros, so
        // the value stays below 10^35 and fits in an i128 without overflow.
        let padding = usize::from(exponent) - frac_digits.len();
        let magnitude = int_part
            .bytes()
            .chain(frac_digits.bytes())
            .chain(std::iter::repeat_n(b'0', padding))
            .fold(0i128, |acc, digit| acc * 10 + i128::from(digit - b'0'));

        if negative && magnitude == 0 {
            return Err(ParseMoneyError::NegativeZero);
        }
        let value = if negative { -magnitude } else { magnitude };
        let amount = i64::try_from(value).map_err(|_| ParseMoneyError::Overflow)?;
        Ok(Money::from_minor(amount, currency))
    }

    /// Formats the amount as a decimal string with exactly the currency's
    /// number of decimal places: 3,920 santim is `"39.20"`.
    /// [`Money::parse`] reads it back to the same value.
    pub fn to_decimal_string(self) -> String {
        let exponent = u32::from(self.currency.exponent());
        let sign = if self.amount < 0 { "-" } else { "" };
        let magnitude = self.amount.unsigned_abs();
        let scale = 10u64.pow(exponent);
        let whole = magnitude / scale;
        if exponent == 0 {
            format!("{sign}{whole}")
        } else {
            let fraction = magnitude % scale;
            let width = usize::from(self.currency.exponent());
            format!("{sign}{whole}.{fraction:0width$}")
        }
    }
}

impl fmt::Display for Money {
    /// For example, `39.20 ETB`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.to_decimal_string(), self.currency)
    }
}

impl PositiveAmount {
    /// Accepts money that is greater than zero.
    pub fn new(money: Money) -> Result<PositiveAmount, MoneyError> {
        if money.is_positive() {
            Ok(PositiveAmount(money))
        } else {
            Err(MoneyError::NotPositive)
        }
    }

    /// Parses a decimal string, as [`Money::parse`], and requires it to be
    /// greater than zero.
    pub fn parse(s: &str, currency: Currency) -> Result<PositiveAmount, ParseMoneyError> {
        let money = Money::parse(s, currency)?;
        PositiveAmount::new(money).map_err(|_| ParseMoneyError::NotPositive)
    }

    pub const fn money(self) -> Money {
        self.0
    }

    pub const fn amount(self) -> i64 {
        self.0.amount
    }

    pub const fn currency(self) -> Currency {
        self.0.currency
    }
}

impl From<PositiveAmount> for Money {
    fn from(amount: PositiveAmount) -> Money {
        amount.0
    }
}

impl fmt::Display for PositiveAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The JSON form: `{"amount": "39.20", "currency": "ETB"}`. The amount is a
/// string because JSON numbers above 2^53 lose precision in JavaScript.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoneyRepr {
    amount: String,
    currency: Currency,
}

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        MoneyRepr {
            amount: self.to_decimal_string(),
            currency: self.currency,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = MoneyRepr::deserialize(deserializer)?;
        Money::parse(&repr.amount, repr.currency).map_err(serde::de::Error::custom)
    }
}

impl Serialize for PositiveAmount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PositiveAmount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let money = Money::deserialize(deserializer)?;
        PositiveAmount::new(money).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ETB: Currency = Currency::ETB;

    fn etb(amount: i64) -> Money {
        Money::from_minor(amount, ETB)
    }

    #[test]
    fn parses_valid_amounts() {
        let cases = [
            ("0", 0),
            ("0.00", 0),
            ("0.5", 50),
            ("0.05", 5),
            ("1", 100),
            ("39.2", 3920),
            ("39.20", 3920),
            ("1000", 100_000),
            ("-0.01", -1),
            ("-39.20", -3920),
        ];
        for (input, expected) in cases {
            assert_eq!(Money::parse(input, ETB), Ok(etb(expected)), "{input}");
        }
    }

    #[test]
    fn rejects_invalid_amounts() {
        use ParseMoneyError::*;
        let too_long = "1".repeat(MAX_DECIMAL_LEN + 1);
        let cases = [
            ("", Empty),
            (too_long.as_str(), TooLong),
            ("+1", InvalidCharacter),
            (" 1", InvalidCharacter),
            ("1 ", InvalidCharacter),
            ("1e5", InvalidCharacter),
            ("1,000", InvalidCharacter),
            ("--1", InvalidCharacter),
            ("1-", InvalidCharacter),
            ("-", InvalidFormat),
            (".5", InvalidFormat),
            ("5.", InvalidFormat),
            ("1.2.3", InvalidFormat),
            ("00", LeadingZero),
            ("007.50", LeadingZero),
            ("0.001", TooManyDecimals { max: 2 }),
            ("39.200", TooManyDecimals { max: 2 }),
            ("-0", NegativeZero),
            ("-0.00", NegativeZero),
        ];
        for (input, error) in cases {
            assert_eq!(Money::parse(input, ETB), Err(error), "{input:?}");
        }
    }

    #[test]
    fn respects_the_currency_exponent() {
        assert_eq!(
            Money::parse("100", Currency::JPY),
            Ok(Money::from_minor(100, Currency::JPY))
        );
        assert_eq!(
            Money::parse("1.0", Currency::JPY),
            Err(ParseMoneyError::TooManyDecimals { max: 0 })
        );
        assert_eq!(
            Money::parse("1.234", Currency::KWD),
            Ok(Money::from_minor(1234, Currency::KWD))
        );
    }

    #[test]
    fn handles_i64_boundaries() {
        let max = etb(i64::MAX);
        let min = etb(i64::MIN);
        assert_eq!(max.to_decimal_string(), "92233720368547758.07");
        assert_eq!(min.to_decimal_string(), "-92233720368547758.08");
        assert_eq!(Money::parse("92233720368547758.07", ETB), Ok(max));
        assert_eq!(Money::parse("-92233720368547758.08", ETB), Ok(min));
        assert_eq!(
            Money::parse("92233720368547758.08", ETB),
            Err(ParseMoneyError::Overflow)
        );
        assert_eq!(
            Money::parse("-92233720368547758.09", ETB),
            Err(ParseMoneyError::Overflow)
        );
        assert_eq!(
            Money::parse(&"9".repeat(MAX_DECIMAL_LEN), ETB),
            Err(ParseMoneyError::Overflow)
        );
        assert_eq!(max.checked_add(etb(1)), Err(MoneyError::Overflow));
        assert_eq!(min.checked_sub(etb(1)), Err(MoneyError::Overflow));
        assert_eq!(min.checked_neg(), Err(MoneyError::Overflow));
        assert_eq!(max.checked_neg(), Ok(etb(-i64::MAX)));
    }

    #[test]
    fn formats_with_exactly_the_currency_decimals() {
        assert_eq!(etb(0).to_decimal_string(), "0.00");
        assert_eq!(etb(5).to_decimal_string(), "0.05");
        assert_eq!(etb(-5).to_decimal_string(), "-0.05");
        assert_eq!(etb(3920).to_decimal_string(), "39.20");
        assert_eq!(
            Money::from_minor(-7, Currency::JPY).to_decimal_string(),
            "-7"
        );
        assert_eq!(
            Money::from_minor(1, Currency::KWD).to_decimal_string(),
            "0.001"
        );
        assert_eq!(etb(3920).to_string(), "39.20 ETB");
    }

    #[test]
    fn arithmetic_is_checked() {
        assert_eq!(etb(3920).checked_add(etb(80)), Ok(etb(4000)));
        assert_eq!(etb(100).checked_sub(etb(250)), Ok(etb(-150)));
        assert_eq!(etb(150).checked_neg(), Ok(etb(-150)));
        let usd = Money::from_minor(1, Currency::USD);
        let mismatch = MoneyError::CurrencyMismatch {
            left: ETB,
            right: Currency::USD,
        };
        assert_eq!(etb(1).checked_add(usd), Err(mismatch));
        assert_eq!(etb(1).checked_sub(usd), Err(mismatch));
    }

    #[test]
    fn sign_helpers() {
        assert!(Money::zero(ETB).is_zero());
        assert!(etb(1).is_positive() && !etb(1).is_negative() && !etb(1).is_zero());
        assert!(etb(-1).is_negative() && !etb(-1).is_positive());
        assert_eq!(etb(7).amount(), 7);
        assert_eq!(etb(7).currency(), ETB);
    }

    #[test]
    fn positive_amount_rejects_zero_and_negative() {
        assert_eq!(PositiveAmount::new(etb(0)), Err(MoneyError::NotPositive));
        assert_eq!(PositiveAmount::new(etb(-1)), Err(MoneyError::NotPositive));
        let one = PositiveAmount::new(etb(1));
        assert!(one.is_ok());
        assert_eq!(
            PositiveAmount::parse("0.00", ETB),
            Err(ParseMoneyError::NotPositive)
        );
        assert_eq!(
            PositiveAmount::parse("-1", ETB),
            Err(ParseMoneyError::NotPositive)
        );
        assert_eq!(
            PositiveAmount::parse("x", ETB),
            Err(ParseMoneyError::InvalidCharacter)
        );
        let amount = PositiveAmount::parse("39.20", ETB);
        assert_eq!(amount.map(PositiveAmount::amount), Ok(3920));
        assert_eq!(amount.map(PositiveAmount::currency), Ok(ETB));
        assert_eq!(amount.map(PositiveAmount::money), Ok(etb(3920)));
        assert_eq!(amount.map(Money::from), Ok(etb(3920)));
        assert_eq!(amount.map(|a| a.to_string()), Ok("39.20 ETB".to_owned()));
    }

    #[test]
    fn errors_have_messages() {
        let usd = Currency::USD;
        let money_errors = [
            (
                MoneyError::CurrencyMismatch {
                    left: ETB,
                    right: usd,
                },
                "currency mismatch: ETB and USD",
            ),
            (MoneyError::Overflow, "amount overflow"),
            (MoneyError::NotPositive, "amount must be greater than zero"),
        ];
        for (error, message) in money_errors {
            assert_eq!(error.to_string(), message);
        }
        use ParseMoneyError::*;
        let parse_errors = [
            (Empty, "amount is empty"),
            (TooLong, "amount is too long"),
            (InvalidCharacter, "amount has an invalid character"),
            (InvalidFormat, "amount is not a valid decimal"),
            (LeadingZero, "amount has a leading zero"),
            (
                TooManyDecimals { max: 2 },
                "amount has more than 2 decimal places",
            ),
            (NegativeZero, "amount is negative zero"),
            (Overflow, "amount is too large"),
            (NotPositive, "amount must be greater than zero"),
        ];
        for (error, message) in parse_errors {
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn json_uses_strings() -> Result<(), serde_json::Error> {
        let json = serde_json::to_string(&etb(3920))?;
        assert_eq!(json, r#"{"amount":"39.20","currency":"ETB"}"#);
        assert_eq!(serde_json::from_str::<Money>(&json)?, etb(3920));

        let positive: PositiveAmount = serde_json::from_str(&json)?;
        assert_eq!(serde_json::to_string(&positive)?, json);

        for bad in [
            r#"{"amount":39.2,"currency":"ETB"}"#,
            r#"{"amount":"0.001","currency":"ETB"}"#,
            r#"{"amount":"1","currency":"XXX"}"#,
            r#"{"amount":"1","currency":"ETB","extra":1}"#,
        ] {
            assert!(serde_json::from_str::<Money>(bad).is_err(), "{bad}");
        }
        assert!(
            serde_json::from_str::<PositiveAmount>(r#"{"amount":"0","currency":"ETB"}"#).is_err()
        );
        assert!(
            serde_json::from_str::<PositiveAmount>(r#"{"amount":"x","currency":"ETB"}"#).is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
mod properties {
    //! Property tests. The case count comes from `PROPTEST_CASES`
    //! (default 256); the step 1.04 exit gate runs them with 1,000,000.

    use proptest::prelude::*;

    use super::*;

    fn currency() -> impl Strategy<Value = Currency> {
        proptest::sample::select(Currency::ALL)
    }

    fn money() -> impl Strategy<Value = Money> {
        (any::<i64>(), currency())
            .prop_map(|(amount, currency)| Money::from_minor(amount, currency))
    }

    proptest! {
        #[test]
        fn parse_of_format_is_identity(m in money()) {
            prop_assert_eq!(Money::parse(&m.to_decimal_string(), m.currency()), Ok(m));
        }

        #[test]
        fn json_round_trips(m in money()) {
            let back = serde_json::to_string(&m).and_then(|json| serde_json::from_str::<Money>(&json));
            prop_assert_eq!(back.ok(), Some(m));
        }

        #[test]
        fn checked_add_matches_wide_arithmetic(a in any::<i64>(), b in any::<i64>(), c in currency()) {
            let exact = i128::from(a) + i128::from(b);
            let result = Money::from_minor(a, c).checked_add(Money::from_minor(b, c));
            match i64::try_from(exact) {
                Ok(sum) => prop_assert_eq!(result, Ok(Money::from_minor(sum, c))),
                Err(_) => prop_assert_eq!(result, Err(MoneyError::Overflow)),
            }
        }

        #[test]
        fn checked_sub_matches_wide_arithmetic(a in any::<i64>(), b in any::<i64>(), c in currency()) {
            let exact = i128::from(a) - i128::from(b);
            let result = Money::from_minor(a, c).checked_sub(Money::from_minor(b, c));
            match i64::try_from(exact) {
                Ok(diff) => prop_assert_eq!(result, Ok(Money::from_minor(diff, c))),
                Err(_) => prop_assert_eq!(result, Err(MoneyError::Overflow)),
            }
        }

        #[test]
        fn mixed_currencies_always_error(
            x in any::<i64>(),
            y in any::<i64>(),
            i in 0..Currency::ALL.len(),
            offset in 1..Currency::ALL.len(),
        ) {
            // Two different currencies by construction, so no case is discarded.
            let left = Currency::ALL[i];
            let right = Currency::ALL[(i + offset) % Currency::ALL.len()];
            let (a, b) = (Money::from_minor(x, left), Money::from_minor(y, right));
            let mismatch = MoneyError::CurrencyMismatch { left: a.currency(), right: b.currency() };
            prop_assert_eq!(a.checked_add(b), Err(mismatch));
            prop_assert_eq!(a.checked_sub(b), Err(mismatch));
        }

        #[test]
        fn parse_never_panics(s in ".{0,40}", c in currency()) {
            let _ = Money::parse(&s, c);
        }

        #[test]
        fn parse_never_panics_on_numeric_looking_input(s in "-?[0-9]{0,25}(\\.[0-9]{0,5})?", c in currency()) {
            if let Ok(m) = Money::parse(&s, c) {
                prop_assert_eq!(Money::parse(&m.to_decimal_string(), c), Ok(m));
            }
        }

        #[test]
        fn positive_amount_accepts_exactly_positive(m in money()) {
            prop_assert_eq!(PositiveAmount::new(m).is_ok(), m.amount() > 0);
        }
    }
}

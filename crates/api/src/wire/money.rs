//! Amounts: decimal strings with their currency.

use mezgeb_core::{Currency, Money};
use serde::{Deserialize, Serialize};

use crate::problem::ApiError;

/// An amount with its currency: `{"amount": "39.20", "currency": "ETB"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoneyJson {
    pub amount: String,
    pub currency: String,
}

impl MoneyJson {
    /// The amount, refusing an unknown currency or more decimal places than
    /// the currency has. `pointer` names the field in errors.
    pub fn to_money(&self, pointer: &str) -> Result<Money, ApiError> {
        let currency = Currency::from_code(&self.currency).map_err(|_| {
            ApiError::invalid(
                &format!("{pointer}/currency"),
                format!("{:?} is not a supported currency", self.currency),
            )
        })?;
        Money::parse(&self.amount, currency).map_err(|e| {
            ApiError::invalid(
                &format!("{pointer}/amount"),
                format!(
                    "{:?} is not a valid {} amount: {e}",
                    self.amount,
                    currency.code()
                ),
            )
        })
    }
}

impl From<Money> for MoneyJson {
    fn from(money: Money) -> MoneyJson {
        MoneyJson {
            amount: money.to_decimal_string(),
            currency: money.currency().code().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_strict_decimal_strings() {
        let etb = |amount: &str| MoneyJson {
            amount: amount.into(),
            currency: "ETB".into(),
        };
        assert_eq!(etb("39.20").to_money("/a").map(Money::amount), Ok(3_920));
        assert_eq!(etb("39.2").to_money("/a").map(Money::amount), Ok(3_920));
        assert_eq!(etb("-5").to_money("/a").map(Money::amount), Ok(-500));
        for bad in [
            "39.205",
            "1e3",
            "",
            "abc",
            " 1",
            "+1",
            "1,000",
            "99999999999999999999",
        ] {
            assert_eq!(
                etb(bad).to_money("/a").map_err(|e| e.code()),
                Err("invalid_request"),
                "{bad:?}"
            );
        }
        let unknown = MoneyJson {
            amount: "1".into(),
            currency: "XXX".into(),
        };
        assert_eq!(
            unknown.to_money("/a").map_err(|e| e.code()),
            Err("invalid_request")
        );
        assert_eq!(
            MoneyJson::from(Money::from_minor(3_920, Currency::ETB)),
            etb("39.20")
        );
    }
}

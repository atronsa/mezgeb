//! An account.

use mezgeb_core::{Account, Money};
use serde::Serialize;

use super::*;

#[derive(Debug, Serialize)]
pub struct AccountJson {
    pub id: String,
    pub reference: String,
    pub code: u16,
    pub sub_account: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub normal_side: &'static str,
    pub currency: &'static str,
    pub status: &'static str,
    pub allow_negative: bool,
    pub balance: MoneyJson,
    pub debits_total: MoneyJson,
    pub credits_total: MoneyJson,
}

impl From<&Account> for AccountJson {
    fn from(a: &Account) -> AccountJson {
        let currency = a.currency();
        AccountJson {
            id: a.id().to_string(),
            reference: a.reference(),
            code: a.code().value(),
            sub_account: a.sub_account().map(str::to_owned),
            name: a.name().to_owned(),
            kind: a.account_type().as_str(),
            normal_side: a.normal_side().as_str(),
            currency: currency.code(),
            status: a.status().as_str(),
            allow_negative: a.allow_negative(),
            balance: a.balance().into(),
            debits_total: Money::from_minor(a.debits_total(), currency).into(),
            credits_total: Money::from_minor(a.credits_total(), currency).into(),
        }
    }
}

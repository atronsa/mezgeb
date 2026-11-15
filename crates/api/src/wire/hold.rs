//! A hold.

use mezgeb_core::{Hold, Money};
use serde::Serialize;

use super::*;

#[derive(Debug, Serialize)]
pub struct HoldJson {
    pub id: String,
    pub account: String,
    pub held_account: String,
    pub amount: MoneyJson,
    pub captured: MoneyJson,
    pub status: &'static str,
    pub expires_at: String,
    pub place_entry: String,
    pub close_entry: Option<String>,
}

impl From<&Hold> for HoldJson {
    fn from(h: &Hold) -> HoldJson {
        let currency = h.amount().currency();
        HoldJson {
            id: h.id().to_string(),
            account: h.account().to_string(),
            held_account: h.held_account().to_string(),
            amount: h.amount().money().into(),
            captured: Money::from_minor(h.captured(), currency).into(),
            status: h.status().as_str(),
            expires_at: format_time(h.expires_at()),
            place_entry: h.place_entry().to_string(),
            close_entry: h.close_entry().map(|e| e.to_string()),
        }
    }
}

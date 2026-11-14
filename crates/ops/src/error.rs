//! Template errors with stable numeric codes.
//!
//! The codes are part of the public API, like the posting codes in
//! `mezgeb_core::PostingError` (1001–1012): a code is never renumbered or
//! reused, and a new rule gets the next free number.
//!
//! | Code | Error | Rule |
//! |---:|---|---|
//! | 2001 | `InvalidTemplateId` | A template ID is `name.vN`: a lowercase name, then a version from 1 |
//! | 2002 | `UnknownTemplate` | The template name is registered |
//! | 2003 | `UnknownVersion` | That version of the template is registered |
//! | 2004 | `MissingAmount` | Every amount the template needs is given |
//! | 2005 | `UnexpectedAmount` | No amount the template does not use is given |
//! | 2006 | `AmountNotPositive` | Every amount is greater than zero |
//! | 2007 | `MissingAccount` | Every role the template books is bound to an account |
//! | 2008 | `UnexpectedAccount` | No role the template does not have is bound |
//! | 2009 | `AccountNotFound` | Every bound account exists |
//! | 2010 | `AccountNotAllowed` | Every bound account is one its role allows (TM-07) |
//! | 2011 | `SameAccount` | No two roles are bound to the same account |
//! | 2012 | `ReservedMetadata` | The request's metadata does not set the template key |
//! | 2013 | `UndeclaredRole` | A template books only roles it declares (a defect in the template) |
//! | 2014 | `AmountNotBalance` | An item that moves whole moves its account's whole balance |
//! | 2015 | `AccountNotNew` | An item that opens its own sub-account opens one never posted to |
//!
//! Rules are checked in this order: 2001–2003, 2004–2006, 2007–2011, then the
//! template's own rules (such as 2014 and 2015, in the order the template
//! checks them), then 2012 and 2013. Within a rule, amounts and roles are
//! checked in the order the template declares them, then any extra names in
//! sorted order, so the same request always gets the same error.
//!
//! A draft that passes every rule here is then validated by the engine with
//! the posting rules (1001–1012), for example for insufficient funds.

use std::fmt;

use mezgeb_core::AccountId;

/// Why a request could not be booked. Names are the template's or the
/// request's names for an amount or a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    InvalidTemplateId(String),
    UnknownTemplate(String),
    UnknownVersion(String),
    MissingAmount(String),
    UnexpectedAmount(String),
    AmountNotPositive(String),
    MissingAccount(String),
    UnexpectedAccount(String),
    AccountNotFound { role: String, account: AccountId },
    AccountNotAllowed { role: String, account: AccountId },
    SameAccount { role: String, other: String },
    ReservedMetadata,
    UndeclaredRole(String),
    AmountNotBalance(String),
    AccountNotNew(String),
}

impl TemplateError {
    /// The stable numeric code.
    pub const fn code(&self) -> u16 {
        match self {
            TemplateError::InvalidTemplateId(_) => 2001,
            TemplateError::UnknownTemplate(_) => 2002,
            TemplateError::UnknownVersion(_) => 2003,
            TemplateError::MissingAmount(_) => 2004,
            TemplateError::UnexpectedAmount(_) => 2005,
            TemplateError::AmountNotPositive(_) => 2006,
            TemplateError::MissingAccount(_) => 2007,
            TemplateError::UnexpectedAccount(_) => 2008,
            TemplateError::AccountNotFound { .. } => 2009,
            TemplateError::AccountNotAllowed { .. } => 2010,
            TemplateError::SameAccount { .. } => 2011,
            TemplateError::ReservedMetadata => 2012,
            TemplateError::UndeclaredRole(_) => 2013,
            TemplateError::AmountNotBalance(_) => 2014,
            TemplateError::AccountNotNew(_) => 2015,
        }
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] ", self.code())?;
        match self {
            TemplateError::InvalidTemplateId(id) => write!(f, "invalid template id {id:?}"),
            TemplateError::UnknownTemplate(id) => write!(f, "unknown template {id:?}"),
            TemplateError::UnknownVersion(id) => write!(f, "unknown template version {id:?}"),
            TemplateError::MissingAmount(name) => write!(f, "amount {name:?} is missing"),
            TemplateError::UnexpectedAmount(name) => {
                write!(f, "amount {name:?} is not used by this template")
            }
            TemplateError::AmountNotPositive(name) => {
                write!(f, "amount {name:?} is not greater than zero")
            }
            TemplateError::MissingAccount(role) => write!(f, "role {role:?} has no account"),
            TemplateError::UnexpectedAccount(role) => {
                write!(f, "role {role:?} is not used by this template")
            }
            TemplateError::AccountNotFound { role, account } => {
                write!(f, "account {account} for role {role:?} does not exist")
            }
            TemplateError::AccountNotAllowed { role, account } => {
                write!(f, "account {account} is not allowed for role {role:?}")
            }
            TemplateError::SameAccount { role, other } => {
                write!(
                    f,
                    "roles {other:?} and {role:?} are bound to the same account"
                )
            }
            TemplateError::ReservedMetadata => {
                write!(f, "metadata key {:?} is reserved", crate::TEMPLATE_KEY)
            }
            TemplateError::UndeclaredRole(role) => {
                write!(f, "the template books undeclared role {role:?}")
            }
            TemplateError::AmountNotBalance(role) => {
                write!(f, "the amount is not the whole balance of role {role:?}")
            }
            TemplateError::AccountNotNew(role) => {
                write!(
                    f,
                    "the account for role {role:?} has already been posted to"
                )
            }
        }
    }
}

impl std::error::Error for TemplateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_in_the_2000_range() -> Result<(), mezgeb_core::IdError> {
        let account: AccountId = "00000000-0000-7000-8000-000000000001".parse()?;
        let all = [
            TemplateError::InvalidTemplateId(String::new()),
            TemplateError::UnknownTemplate(String::new()),
            TemplateError::UnknownVersion(String::new()),
            TemplateError::MissingAmount(String::new()),
            TemplateError::UnexpectedAmount(String::new()),
            TemplateError::AmountNotPositive(String::new()),
            TemplateError::MissingAccount(String::new()),
            TemplateError::UnexpectedAccount(String::new()),
            TemplateError::AccountNotFound {
                role: String::new(),
                account,
            },
            TemplateError::AccountNotAllowed {
                role: String::new(),
                account,
            },
            TemplateError::SameAccount {
                role: String::new(),
                other: String::new(),
            },
            TemplateError::ReservedMetadata,
            TemplateError::UndeclaredRole(String::new()),
            TemplateError::AmountNotBalance(String::new()),
            TemplateError::AccountNotNew(String::new()),
        ];
        let codes: Vec<u16> = all.iter().map(TemplateError::code).collect();
        assert_eq!(codes, (2001..=2015).collect::<Vec<u16>>());
        for error in &all {
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("[{}] ", error.code()))
            );
        }
        Ok(())
    }
}

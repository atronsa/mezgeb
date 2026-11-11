//! The chart of accounts (docs/accounting/chart-of-accounts.md).
//!
//! A chart is data, not code: an operator loads it from a TOML file such as
//! `charts/wallet-provider.toml`, so the engine stays generic. This module
//! parses and validates the text; reading the file is the caller's job, so
//! `mezgeb-core` stays free of I/O.

use std::collections::HashSet;
use std::fmt;

use serde::Deserialize;

use crate::currency::Currency;

/// Debit or credit: the side of a journal line, and the side on which an
/// account type normally carries its balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Debit,
    Credit,
}

/// The five account types of double-entry bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

impl AccountType {
    /// Every account type.
    pub const ALL: [AccountType; 5] = [
        AccountType::Asset,
        AccountType::Liability,
        AccountType::Equity,
        AccountType::Revenue,
        AccountType::Expense,
    ];

    /// The side on which the balance normally sits and which increases it:
    /// debit for assets and expenses, credit for the rest.
    pub const fn normal_side(self) -> Side {
        match self {
            AccountType::Asset | AccountType::Expense => Side::Debit,
            AccountType::Liability | AccountType::Equity | AccountType::Revenue => Side::Credit,
        }
    }

    /// The type a code range implies, or `None` for the 9xxx range, where
    /// each account states its own type.
    const fn for_range(first_digit: u16) -> Option<AccountType> {
        match first_digit {
            1 => Some(AccountType::Asset),
            2 => Some(AccountType::Liability),
            3 => Some(AccountType::Equity),
            4 => Some(AccountType::Revenue),
            5 => Some(AccountType::Expense),
            _ => None,
        }
    }
}

/// Whether entries post to the account itself or to its sub-accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    /// A family of sub-accounts, one per customer, merchant, withdrawal and
    /// so on (`2010:cust-abebe`). Never posted to directly (INV-24).
    Control,
    /// A single account that entries post to directly (`4010`).
    System,
}

/// Whether a chart account may be used yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartStatus {
    /// In use.
    Active,
    /// Code and type fixed, but how it is posted is still being decided.
    /// Accounts cannot be opened under it yet.
    Reserved,
}

/// A four-digit chart code, 1000–9999. The first digit gives the type,
/// except in the 9xxx suspense and clearing range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountCode(u16);

impl AccountCode {
    /// Accepts 1000–5999 and 9000–9999. 6000–8999 are unassigned.
    pub fn new(code: u16) -> Result<AccountCode, ChartError> {
        match code / 1000 {
            1..=5 | 9 => Ok(AccountCode(code)),
            _ => Err(ChartError::CodeOutOfRange(code)),
        }
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

impl fmt::Display for AccountCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One account in the chart: a system account, or the template for a
/// control account's sub-accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountTemplate {
    code: AccountCode,
    name: String,
    account_type: AccountType,
    kind: AccountKind,
    prefix: Option<String>,
    allow_negative: bool,
    status: ChartStatus,
}

impl AccountTemplate {
    pub const fn code(&self) -> AccountCode {
        self.code
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn account_type(&self) -> AccountType {
        self.account_type
    }

    pub const fn normal_side(&self) -> Side {
        self.account_type.normal_side()
    }

    pub const fn kind(&self) -> AccountKind {
        self.kind
    }

    /// The sub-account prefix of a control account, such as `cust`.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    /// Whether accounts opened under this template may go below zero on
    /// their normal side (INV-8). Off unless the chart says otherwise.
    pub const fn allow_negative(&self) -> bool {
        self.allow_negative
    }

    pub const fn status(&self) -> ChartStatus {
        self.status
    }
}

/// A validated chart of accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chart {
    name: String,
    currency: Currency,
    accounts: Vec<AccountTemplate>,
}

/// Errors from loading a chart or resolving an account reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChartError {
    /// The TOML could not be parsed.
    Toml(String),
    UnknownCurrency(String),
    EmptyChartName,
    CodeOutOfRange(u16),
    /// Codes go in steps of 10, leaving room to insert accounts later.
    CodeNotMultipleOfTen(u16),
    DuplicateCode(u16),
    EmptyName(u16),
    /// The type does not match the code range (for example, a revenue
    /// account coded 2xxx).
    TypeDoesNotMatchRange(u16),
    ControlWithoutPrefix(u16),
    PrefixOnSystemAccount(u16),
    /// Prefixes are 2–8 lowercase ASCII letters.
    InvalidPrefix(u16),
    DuplicatePrefix(String),
    /// A reference such as `"2010:cust-abebe"` is not well formed.
    MalformedReference(String),
    UnknownAccount(u16),
    /// A control account was referenced without a sub-account.
    SubAccountRequired(u16),
    /// A system account was referenced with a sub-account.
    SubAccountNotAllowed(u16),
    /// The sub-account does not start with the account's prefix and `-`,
    /// or has characters other than lowercase letters, digits and `-`.
    InvalidSubAccount(String),
    /// The account is reserved and cannot be opened yet.
    ReservedAccount(u16),
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChartError::Toml(message) => write!(f, "invalid chart file: {message}"),
            ChartError::UnknownCurrency(code) => write!(f, "unknown currency {code:?}"),
            ChartError::EmptyChartName => f.write_str("chart name is empty"),
            ChartError::CodeOutOfRange(code) => write!(f, "account code {code} is out of range"),
            ChartError::CodeNotMultipleOfTen(code) => {
                write!(f, "account code {code} is not a multiple of 10")
            }
            ChartError::DuplicateCode(code) => write!(f, "account code {code} is used twice"),
            ChartError::EmptyName(code) => write!(f, "account {code} has no name"),
            ChartError::TypeDoesNotMatchRange(code) => {
                write!(
                    f,
                    "account {code} has a type that does not match its code range"
                )
            }
            ChartError::ControlWithoutPrefix(code) => {
                write!(f, "control account {code} has no sub-account prefix")
            }
            ChartError::PrefixOnSystemAccount(code) => {
                write!(f, "system account {code} must not have a prefix")
            }
            ChartError::InvalidPrefix(code) => {
                write!(f, "account {code} has an invalid prefix")
            }
            ChartError::DuplicatePrefix(prefix) => write!(f, "prefix {prefix:?} is used twice"),
            ChartError::MalformedReference(reference) => {
                write!(f, "malformed account reference {reference:?}")
            }
            ChartError::UnknownAccount(code) => write!(f, "account {code} is not in the chart"),
            ChartError::SubAccountRequired(code) => {
                write!(f, "control account {code} needs a sub-account")
            }
            ChartError::SubAccountNotAllowed(code) => {
                write!(f, "system account {code} has no sub-accounts")
            }
            ChartError::InvalidSubAccount(sub) => write!(f, "invalid sub-account {sub:?}"),
            ChartError::ReservedAccount(code) => {
                write!(f, "account {code} is reserved and cannot be used yet")
            }
        }
    }
}

impl std::error::Error for ChartError {}

/// The file format. Converted into a [`Chart`] only after validation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChartFile {
    name: String,
    currency: String,
    accounts: Vec<AccountFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountFile {
    code: u16,
    name: String,
    #[serde(rename = "type")]
    account_type: AccountType,
    kind: AccountKind,
    prefix: Option<String>,
    #[serde(default)]
    allow_negative: bool,
    status: ChartStatus,
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
}

fn valid_prefix(prefix: &str) -> bool {
    (2..=8).contains(&prefix.len()) && prefix.bytes().all(|b| b.is_ascii_lowercase())
}

fn validate_account(file: AccountFile) -> Result<AccountTemplate, ChartError> {
    let code = AccountCode::new(file.code)?;
    let raw = code.value();
    if raw % 10 != 0 {
        return Err(ChartError::CodeNotMultipleOfTen(raw));
    }
    if file.name.trim().is_empty() {
        return Err(ChartError::EmptyName(raw));
    }
    if AccountType::for_range(raw / 1000).is_some_and(|t| t != file.account_type) {
        return Err(ChartError::TypeDoesNotMatchRange(raw));
    }
    match (file.kind, file.prefix.as_deref()) {
        (AccountKind::Control, None) => return Err(ChartError::ControlWithoutPrefix(raw)),
        (AccountKind::System, Some(_)) => return Err(ChartError::PrefixOnSystemAccount(raw)),
        (AccountKind::Control, Some(prefix)) if !valid_prefix(prefix) => {
            return Err(ChartError::InvalidPrefix(raw));
        }
        _ => {}
    }
    Ok(AccountTemplate {
        code,
        name: file.name,
        account_type: file.account_type,
        kind: file.kind,
        prefix: file.prefix,
        allow_negative: file.allow_negative,
        status: file.status,
    })
}

impl Chart {
    /// Parses and validates a chart from TOML text.
    pub fn from_toml(text: &str) -> Result<Chart, ChartError> {
        let file: ChartFile = toml::from_str(text).map_err(|e| ChartError::Toml(e.to_string()))?;
        if file.name.trim().is_empty() {
            return Err(ChartError::EmptyChartName);
        }
        let currency = Currency::from_code(&file.currency)
            .map_err(|_| ChartError::UnknownCurrency(file.currency.clone()))?;
        let mut codes = HashSet::new();
        let mut prefixes = HashSet::new();
        let mut accounts = Vec::with_capacity(file.accounts.len());
        for account in file.accounts {
            let template = validate_account(account)?;
            if !codes.insert(template.code) {
                return Err(ChartError::DuplicateCode(template.code.value()));
            }
            if let Some(prefix) = template.prefix()
                && !prefixes.insert(prefix.to_owned())
            {
                return Err(ChartError::DuplicatePrefix(prefix.to_owned()));
            }
            accounts.push(template);
        }
        accounts.sort_by_key(|a| a.code);
        Ok(Chart {
            name: file.name,
            currency,
            accounts,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The chart's currency. All accounts in v0 use it (step 3.08 adds
    /// more currencies).
    pub const fn currency(&self) -> Currency {
        self.currency
    }

    /// Every account, sorted by code.
    pub fn accounts(&self) -> &[AccountTemplate] {
        &self.accounts
    }

    /// The account with this code, if it is in the chart.
    pub fn account(&self, code: AccountCode) -> Option<&AccountTemplate> {
        self.accounts
            .binary_search_by_key(&code, |a| a.code)
            .ok()
            .and_then(|i| self.accounts.get(i))
    }

    /// Resolves a reference as written in the worked entries: `"4010"` for a
    /// system account, `"2010:cust-abebe"` for a sub-account. Returns the
    /// template and the sub-account, if any. Reserved accounts are rejected.
    pub fn resolve<'a>(
        &self,
        reference: &'a str,
    ) -> Result<(&AccountTemplate, Option<&'a str>), ChartError> {
        let (code_text, sub) = match reference.split_once(':') {
            Some((code_text, sub)) => (code_text, Some(sub)),
            None => (reference, None),
        };
        let malformed = || ChartError::MalformedReference(reference.to_owned());
        if code_text.len() != 4 || !code_text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed());
        }
        // Four ASCII digits, so at most 9999: no overflow.
        let raw = code_text
            .bytes()
            .fold(0u16, |acc, digit| acc * 10 + u16::from(digit - b'0'));
        let code = AccountCode::new(raw)?;
        let template = self.account(code).ok_or(ChartError::UnknownAccount(raw))?;
        if template.status == ChartStatus::Reserved {
            return Err(ChartError::ReservedAccount(raw));
        }
        match (template.prefix(), sub) {
            (None, None) => {}
            (Some(_), None) => return Err(ChartError::SubAccountRequired(raw)),
            (None, Some(_)) => return Err(ChartError::SubAccountNotAllowed(raw)),
            (Some(prefix), Some(sub)) => validate_sub_account(prefix, sub)?,
        }
        Ok((template, sub))
    }
}

/// A sub-account is `<prefix>-<id>`: the control account's prefix, a dash,
/// then lowercase letters, digits and dashes, 64 characters at most.
fn validate_sub_account(prefix: &str, sub: &str) -> Result<(), ChartError> {
    let id = sub
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
        .unwrap_or("");
    let valid = !id.is_empty()
        && sub.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(ChartError::InvalidSubAccount(sub.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart(accounts: &str) -> Result<Chart, ChartError> {
        Chart::from_toml(&format!("name = \"Test\"\ncurrency = \"ETB\"\n{accounts}"))
    }

    const SYSTEM: &str = r#"
[[accounts]]
code = 1010
name = "Bank"
type = "asset"
kind = "system"
status = "active"
"#;

    const CONTROL: &str = r#"
[[accounts]]
code = 2010
name = "Customer wallets"
type = "liability"
kind = "control"
prefix = "cust"
status = "active"
"#;

    fn account(code: u16, account_type: &str, kind: &str, extra: &str) -> String {
        format!(
            "[[accounts]]\ncode = {code}\nname = \"X\"\ntype = \"{account_type}\"\nkind = \"{kind}\"\nstatus = \"active\"\n{extra}\n"
        )
    }

    #[test]
    fn normal_sides_follow_the_type() {
        assert_eq!(AccountType::Asset.normal_side(), Side::Debit);
        assert_eq!(AccountType::Expense.normal_side(), Side::Debit);
        assert_eq!(AccountType::Liability.normal_side(), Side::Credit);
        assert_eq!(AccountType::Equity.normal_side(), Side::Credit);
        assert_eq!(AccountType::Revenue.normal_side(), Side::Credit);
    }

    #[test]
    fn loads_a_valid_chart() -> Result<(), ChartError> {
        let chart = chart(&format!("{CONTROL}{SYSTEM}"))?;
        assert_eq!(chart.name(), "Test");
        assert_eq!(chart.currency(), Currency::ETB);
        let codes: Vec<u16> = chart.accounts().iter().map(|a| a.code().value()).collect();
        assert_eq!(codes, [1010, 2010], "sorted by code");
        let wallets = chart.account(AccountCode::new(2010)?);
        assert_eq!(wallets.map(AccountTemplate::name), Some("Customer wallets"));
        assert_eq!(wallets.and_then(AccountTemplate::prefix), Some("cust"));
        assert_eq!(
            wallets.map(AccountTemplate::kind),
            Some(AccountKind::Control)
        );
        assert_eq!(
            wallets.map(AccountTemplate::normal_side),
            Some(Side::Credit)
        );
        assert_eq!(wallets.map(AccountTemplate::allow_negative), Some(false));
        assert_eq!(
            wallets.map(AccountTemplate::status),
            Some(ChartStatus::Active)
        );
        assert_eq!(chart.account(AccountCode::new(3010)?), None);
        Ok(())
    }

    #[test]
    fn rejects_invalid_charts() {
        let cases: Vec<(String, ChartError)> = vec![
            ("accounts = 5".into(), ChartError::Toml(String::new())),
            (format!("{SYSTEM}{SYSTEM}"), ChartError::DuplicateCode(1010)),
            (
                account(6010, "asset", "system", ""),
                ChartError::CodeOutOfRange(6010),
            ),
            (
                account(999, "asset", "system", ""),
                ChartError::CodeOutOfRange(999),
            ),
            (
                account(1015, "asset", "system", ""),
                ChartError::CodeNotMultipleOfTen(1015),
            ),
            (
                account(4010, "liability", "system", ""),
                ChartError::TypeDoesNotMatchRange(4010),
            ),
            (
                account(2010, "liability", "control", ""),
                ChartError::ControlWithoutPrefix(2010),
            ),
            (
                account(1010, "asset", "system", "prefix = \"bank\""),
                ChartError::PrefixOnSystemAccount(1010),
            ),
            (
                account(2010, "liability", "control", "prefix = \"Cust\""),
                ChartError::InvalidPrefix(2010),
            ),
            (
                account(2010, "liability", "control", "prefix = \"c\""),
                ChartError::InvalidPrefix(2010),
            ),
            (
                format!(
                    "{}{}",
                    account(2010, "liability", "control", "prefix = \"cust\""),
                    account(2020, "liability", "control", "prefix = \"cust\"")
                ),
                ChartError::DuplicatePrefix("cust".into()),
            ),
            (
                account(1010, "asset", "system", "").replace("name = \"X\"", "name = \" \""),
                ChartError::EmptyName(1010),
            ),
        ];
        for (accounts, expected) in cases {
            let result = chart(&accounts);
            match (&result, &expected) {
                (Err(ChartError::Toml(_)), ChartError::Toml(_)) => {}
                _ => assert_eq!(result, Err(expected), "{accounts}"),
            }
        }
        assert_eq!(
            Chart::from_toml("name = \"\"\ncurrency = \"ETB\"\naccounts = []"),
            Err(ChartError::EmptyChartName)
        );
        assert_eq!(
            Chart::from_toml("name = \"T\"\ncurrency = \"XXX\"\naccounts = []"),
            Err(ChartError::UnknownCurrency("XXX".into()))
        );
    }

    #[test]
    fn suspense_range_accepts_any_type() -> Result<(), ChartError> {
        let chart = chart(&format!(
            "{}{}",
            account(9010, "liability", "system", ""),
            account(9020, "asset", "system", "")
        ))?;
        assert_eq!(chart.accounts().len(), 2);
        Ok(())
    }

    #[test]
    fn resolves_references() -> Result<(), ChartError> {
        let reserved = account(5020, "expense", "system", "").replace("active", "reserved");
        let chart = chart(&format!("{SYSTEM}{CONTROL}{reserved}"))?;

        let (bank, sub) = chart.resolve("1010")?;
        assert_eq!((bank.code().value(), sub), (1010, None));
        let (wallets, sub) = chart.resolve("2010:cust-abebe")?;
        assert_eq!((wallets.code().value(), sub), (2010, Some("cust-abebe")));
        assert!(chart.resolve("2010:cust-wd-0001-x").is_ok());

        let cases = [
            ("", ChartError::MalformedReference("".into())),
            ("101", ChartError::MalformedReference("101".into())),
            ("10100", ChartError::MalformedReference("10100".into())),
            ("1o10", ChartError::MalformedReference("1o10".into())),
            ("+101", ChartError::MalformedReference("+101".into())),
            ("7010", ChartError::CodeOutOfRange(7010)),
            ("3010", ChartError::UnknownAccount(3010)),
            ("5020", ChartError::ReservedAccount(5020)),
            ("2010", ChartError::SubAccountRequired(2010)),
            ("1010:cust-x", ChartError::SubAccountNotAllowed(1010)),
            (
                "2010:merch-x",
                ChartError::InvalidSubAccount("merch-x".into()),
            ),
            ("2010:cust-", ChartError::InvalidSubAccount("cust-".into())),
            ("2010:cust", ChartError::InvalidSubAccount("cust".into())),
            (
                "2010:custabebe",
                ChartError::InvalidSubAccount("custabebe".into()),
            ),
            (
                "2010:cust-Abebe",
                ChartError::InvalidSubAccount("cust-Abebe".into()),
            ),
        ];
        for (reference, expected) in cases {
            assert_eq!(
                chart.resolve(reference).err(),
                Some(expected),
                "{reference}"
            );
        }
        let long = format!("cust-{}", "a".repeat(60));
        assert_eq!(
            chart.resolve(&format!("2010:{long}")).err(),
            Some(ChartError::InvalidSubAccount(long))
        );
        Ok(())
    }

    #[test]
    fn errors_have_messages() {
        let cases = [
            (ChartError::Toml("x".into()), "invalid chart file: x"),
            (
                ChartError::UnknownCurrency("XXX".into()),
                "unknown currency \"XXX\"",
            ),
            (ChartError::EmptyChartName, "chart name is empty"),
            (
                ChartError::CodeOutOfRange(7),
                "account code 7 is out of range",
            ),
            (
                ChartError::CodeNotMultipleOfTen(1015),
                "account code 1015 is not a multiple of 10",
            ),
            (
                ChartError::DuplicateCode(1010),
                "account code 1010 is used twice",
            ),
            (ChartError::EmptyName(1010), "account 1010 has no name"),
            (
                ChartError::TypeDoesNotMatchRange(4010),
                "account 4010 has a type that does not match its code range",
            ),
            (
                ChartError::ControlWithoutPrefix(2010),
                "control account 2010 has no sub-account prefix",
            ),
            (
                ChartError::PrefixOnSystemAccount(1010),
                "system account 1010 must not have a prefix",
            ),
            (
                ChartError::InvalidPrefix(2010),
                "account 2010 has an invalid prefix",
            ),
            (
                ChartError::DuplicatePrefix("cust".into()),
                "prefix \"cust\" is used twice",
            ),
            (
                ChartError::MalformedReference("x".into()),
                "malformed account reference \"x\"",
            ),
            (
                ChartError::UnknownAccount(3010),
                "account 3010 is not in the chart",
            ),
            (
                ChartError::SubAccountRequired(2010),
                "control account 2010 needs a sub-account",
            ),
            (
                ChartError::SubAccountNotAllowed(1010),
                "system account 1010 has no sub-accounts",
            ),
            (
                ChartError::InvalidSubAccount("x".into()),
                "invalid sub-account \"x\"",
            ),
            (
                ChartError::ReservedAccount(5020),
                "account 5020 is reserved and cannot be used yet",
            ),
        ];
        for (error, message) in cases {
            assert_eq!(error.to_string(), message);
        }
        assert_eq!(AccountCode(1010).to_string(), "1010");
    }
}

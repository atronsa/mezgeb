//! Accounts and their lifecycle.
//!
//! An account is opened from a [`Chart`] reference such as `"1010"` or
//! `"2010:cust-abebe"`, so its type, normal side and negative-balance rule
//! always come from the chart and can never disagree with it.

use std::fmt;

use crate::chart::{AccountCode, AccountType, Chart, ChartError, Side};
use crate::currency::Currency;
use crate::ids::AccountId;
use crate::money::Money;

/// Where an account is in its lifecycle.
///
/// ```text
/// Open ⇄ Frozen
///   └──────┴──▶ Closed   (only at zero balance; never reopened)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountStatus {
    /// Accepts postings.
    Open,
    /// Temporarily blocked, for example during an investigation. Accepts no
    /// postings; can be reopened.
    Frozen,
    /// Permanently closed. Accepts no postings and can never reopen.
    Closed,
}

/// Why a status change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusError {
    /// The account is already in that status.
    Unchanged(AccountStatus),
    /// A closed account can never change status again.
    AccountClosed,
    /// An account can only be closed when its balance is zero.
    BalanceNotZero,
}

impl fmt::Display for AccountStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusError::Unchanged(status) => write!(f, "account is already {status}"),
            StatusError::AccountClosed => f.write_str("a closed account cannot change status"),
            StatusError::BalanceNotZero => f.write_str("an account can only close at zero balance"),
        }
    }
}

impl std::error::Error for StatusError {}

impl AccountStatus {
    /// The name used in storage and the API: `"open"`, `"frozen"`, `"closed"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            AccountStatus::Open => "open",
            AccountStatus::Frozen => "frozen",
            AccountStatus::Closed => "closed",
        }
    }

    /// The inverse of [`AccountStatus::as_str`].
    pub fn from_name(name: &str) -> Option<AccountStatus> {
        AccountStatus::ALL.into_iter().find(|s| s.as_str() == name)
    }

    /// Every status.
    pub const ALL: [AccountStatus; 3] = [
        AccountStatus::Open,
        AccountStatus::Frozen,
        AccountStatus::Closed,
    ];

    /// The status after changing to `to`, or why the change is refused.
    pub fn transition(
        self,
        to: AccountStatus,
        balance_is_zero: bool,
    ) -> Result<AccountStatus, StatusError> {
        if self == to {
            return Err(StatusError::Unchanged(to));
        }
        match (self, to) {
            (AccountStatus::Closed, _) => Err(StatusError::AccountClosed),
            (_, AccountStatus::Closed) if !balance_is_zero => Err(StatusError::BalanceNotZero),
            _ => Ok(to),
        }
    }

    /// Only open accounts accept postings (INV-24).
    pub const fn accepts_postings(self) -> bool {
        matches!(self, AccountStatus::Open)
    }
}

/// An account's balance on its normal side: positive when the balance sits
/// on the side its type normally carries, negative otherwise. `None` only if
/// the result does not fit in an `i64`, which cannot happen for the
/// non-negative totals the ledger stores.
pub const fn balance(normal_side: Side, debits_total: i64, credits_total: i64) -> Option<i64> {
    match normal_side {
        Side::Debit => debits_total.checked_sub(credits_total),
        Side::Credit => credits_total.checked_sub(debits_total),
    }
}

/// An account as stored, for rebuilding an [`Account`] from a database row
/// with [`Account::from_record`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub id: AccountId,
    pub code: AccountCode,
    pub sub_account: Option<String>,
    pub name: String,
    pub account_type: AccountType,
    pub currency: Currency,
    pub allow_negative: bool,
    pub status: AccountStatus,
    pub debits_total: i64,
    pub credits_total: i64,
}

/// A stored account that breaks the account rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// A total is below zero.
    NegativeTotal,
    /// The balance is below zero and the account does not allow it (INV-8).
    NegativeBalance,
    /// A closed account has a non-zero balance.
    ClosedWithBalance,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RecordError::NegativeTotal => "stored account has a negative total",
            RecordError::NegativeBalance => "stored account has a forbidden negative balance",
            RecordError::ClosedWithBalance => "stored closed account has a balance",
        })
    }
}

impl std::error::Error for RecordError {}

/// A ledger account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    id: AccountId,
    code: AccountCode,
    sub_account: Option<String>,
    name: String,
    account_type: AccountType,
    currency: Currency,
    allow_negative: bool,
    status: AccountStatus,
    /// Both totals are in `0..=i64::MAX`, so their difference always fits
    /// in an `i64` (ADR-0012).
    debits_total: i64,
    credits_total: i64,
}

impl Account {
    /// Opens a new account, with zero totals, for a chart reference.
    pub fn open(id: AccountId, chart: &Chart, reference: &str) -> Result<Account, ChartError> {
        let (template, sub_account) = chart.resolve(reference)?;
        Ok(Account {
            id,
            code: template.code(),
            sub_account: sub_account.map(str::to_owned),
            name: template.name().to_owned(),
            account_type: template.account_type(),
            currency: chart.currency(),
            allow_negative: template.allow_negative(),
            status: AccountStatus::Open,
            debits_total: 0,
            credits_total: 0,
        })
    }

    /// Rebuilds an account from storage, refusing a record that breaks the
    /// account rules, so a corrupted row can never be used for posting.
    pub fn from_record(record: AccountRecord) -> Result<Account, RecordError> {
        if record.debits_total < 0 || record.credits_total < 0 {
            return Err(RecordError::NegativeTotal);
        }
        let account = Account {
            id: record.id,
            code: record.code,
            sub_account: record.sub_account,
            name: record.name,
            account_type: record.account_type,
            currency: record.currency,
            allow_negative: record.allow_negative,
            status: record.status,
            debits_total: record.debits_total,
            credits_total: record.credits_total,
        };
        let balance = account.balance();
        if balance.is_negative() && !account.allow_negative {
            return Err(RecordError::NegativeBalance);
        }
        if account.status == AccountStatus::Closed && !balance.is_zero() {
            return Err(RecordError::ClosedWithBalance);
        }
        Ok(account)
    }

    pub const fn id(&self) -> AccountId {
        self.id
    }

    pub const fn code(&self) -> AccountCode {
        self.code
    }

    /// The sub-account, such as `cust-abebe`, for an account under a
    /// control account.
    pub fn sub_account(&self) -> Option<&str> {
        self.sub_account.as_deref()
    }

    /// The reference as written in the worked entries: `"1010"` or
    /// `"2010:cust-abebe"`.
    pub fn reference(&self) -> String {
        match &self.sub_account {
            Some(sub) => format!("{}:{sub}", self.code),
            None => self.code.to_string(),
        }
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

    pub const fn currency(&self) -> Currency {
        self.currency
    }

    pub const fn allow_negative(&self) -> bool {
        self.allow_negative
    }

    pub const fn status(&self) -> AccountStatus {
        self.status
    }

    pub const fn debits_total(&self) -> i64 {
        self.debits_total
    }

    pub const fn credits_total(&self) -> i64 {
        self.credits_total
    }

    /// The balance on the account's normal side, in its currency.
    pub const fn balance(&self) -> Money {
        // Totals are non-negative, so this subtraction cannot overflow.
        let amount = match self.normal_side() {
            Side::Debit => self.debits_total - self.credits_total,
            Side::Credit => self.credits_total - self.debits_total,
        };
        Money::from_minor(amount, self.currency)
    }

    /// Changes the status, following [`AccountStatus::transition`].
    pub fn set_status(&mut self, to: AccountStatus) -> Result<(), StatusError> {
        self.status = self.status.transition(to, self.balance().is_zero())?;
        Ok(())
    }

    /// Adds one posted line to the totals. Only ledgers inside this crate
    /// call it, after `validate` has checked the entry; the totals stay in
    /// `0..=i64::MAX` or nothing changes.
    pub(crate) fn record(&mut self, side: Side, amount: i64) -> Option<()> {
        let total = match side {
            Side::Debit => &mut self.debits_total,
            Side::Credit => &mut self.credits_total,
        };
        *total = total.checked_add(amount).filter(|t| *t >= 0)?;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AccountStatus::*;

    const CHART: &str = r#"
name = "Test"
currency = "ETB"

[[accounts]]
code = 1010
name = "Bank: customer funds account"
type = "asset"
kind = "system"
status = "active"

[[accounts]]
code = 2010
name = "Customer wallets"
type = "liability"
kind = "control"
prefix = "cust"
status = "active"
"#;

    fn id() -> Result<AccountId, crate::ids::IdError> {
        "017f22e2-79b0-7cc3-98c4-dc0c0c07398f".parse()
    }

    #[test]
    fn every_status_transition() {
        // (from, to, balance is zero, expected)
        let cases = [
            (Open, Open, true, Err(StatusError::Unchanged(Open))),
            (Open, Frozen, true, Ok(Frozen)),
            (Open, Frozen, false, Ok(Frozen)),
            (Open, Closed, true, Ok(Closed)),
            (Open, Closed, false, Err(StatusError::BalanceNotZero)),
            (Frozen, Open, true, Ok(Open)),
            (Frozen, Open, false, Ok(Open)),
            (Frozen, Frozen, true, Err(StatusError::Unchanged(Frozen))),
            (Frozen, Closed, true, Ok(Closed)),
            (Frozen, Closed, false, Err(StatusError::BalanceNotZero)),
            (Closed, Open, true, Err(StatusError::AccountClosed)),
            (Closed, Frozen, true, Err(StatusError::AccountClosed)),
            (Closed, Closed, true, Err(StatusError::Unchanged(Closed))),
        ];
        for (from, to, zero, expected) in cases {
            assert_eq!(
                from.transition(to, zero),
                expected,
                "{from} -> {to}, zero: {zero}"
            );
        }
    }

    #[test]
    fn only_open_accounts_accept_postings() {
        assert!(Open.accepts_postings());
        assert!(!Frozen.accepts_postings());
        assert!(!Closed.accepts_postings());
    }

    #[test]
    fn opens_accounts_from_the_chart() -> Result<(), Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(CHART)?;
        let wallet = Account::open(id()?, &chart, "2010:cust-abebe")?;
        assert_eq!(wallet.id(), id()?);
        assert_eq!(wallet.code().value(), 2010);
        assert_eq!(wallet.sub_account(), Some("cust-abebe"));
        assert_eq!(wallet.reference(), "2010:cust-abebe");
        assert_eq!(wallet.name(), "Customer wallets");
        assert_eq!(wallet.account_type(), AccountType::Liability);
        assert_eq!(wallet.normal_side(), Side::Credit);
        assert_eq!(wallet.currency(), Currency::ETB);
        assert!(!wallet.allow_negative());
        assert_eq!(wallet.status(), Open);
        assert_eq!((wallet.debits_total(), wallet.credits_total()), (0, 0));
        assert_eq!(wallet.balance(), Money::zero(Currency::ETB));

        let bank = Account::open(id()?, &chart, "1010")?;
        assert_eq!(bank.reference(), "1010");
        assert_eq!(bank.sub_account(), None);
        assert_eq!(bank.normal_side(), Side::Debit);

        assert_eq!(
            Account::open(id()?, &chart, "2010"),
            Err(ChartError::SubAccountRequired(2010))
        );
        Ok(())
    }

    #[test]
    fn balance_follows_the_normal_side() -> Result<(), Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(CHART)?;
        let mut bank = Account::open(id()?, &chart, "1010")?;
        let mut wallet = Account::open(id()?, &chart, "2010:cust-abebe")?;
        // E01: Dr 1010 1,000 / Cr 2010:cust-abebe 1,000.
        bank.debits_total = 100_000;
        wallet.credits_total = 100_000;
        assert_eq!(bank.balance(), Money::from_minor(100_000, Currency::ETB));
        assert_eq!(wallet.balance(), Money::from_minor(100_000, Currency::ETB));
        // A wallet debited more than credited is negative on its normal side.
        wallet.debits_total = 100_001;
        assert_eq!(wallet.balance(), Money::from_minor(-1, Currency::ETB));
        Ok(())
    }

    #[test]
    fn accounts_close_only_at_zero() -> Result<(), Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(CHART)?;
        let mut wallet = Account::open(id()?, &chart, "2010:cust-abebe")?;
        wallet.credits_total = 500;
        assert_eq!(wallet.set_status(Closed), Err(StatusError::BalanceNotZero));
        wallet.set_status(Frozen)?;
        wallet.set_status(Open)?;
        wallet.debits_total = 500;
        wallet.set_status(Closed)?;
        assert_eq!(wallet.status(), Closed);
        assert_eq!(wallet.set_status(Open), Err(StatusError::AccountClosed));
        Ok(())
    }

    #[test]
    fn status_names_round_trip() {
        for status in AccountStatus::ALL {
            assert_eq!(AccountStatus::from_name(status.as_str()), Some(status));
        }
        assert_eq!(AccountStatus::from_name("Open"), None);
    }

    #[test]
    fn records_are_checked_when_restored() -> Result<(), Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(CHART)?;
        let wallet = Account::open(id()?, &chart, "2010:cust-abebe")?;
        let record = |debits: i64, credits: i64, status: AccountStatus, allow_negative: bool| {
            AccountRecord {
                id: wallet.id(),
                code: wallet.code(),
                sub_account: wallet.sub_account().map(str::to_owned),
                name: wallet.name().to_owned(),
                account_type: wallet.account_type(),
                currency: wallet.currency(),
                allow_negative,
                status,
                debits_total: debits,
                credits_total: credits,
            }
        };
        let restored = Account::from_record(record(300, 1_000, Open, false))?;
        assert_eq!(restored.balance(), Money::from_minor(700, Currency::ETB));
        assert_eq!(
            Account::from_record(record(-1, 0, Open, false)),
            Err(RecordError::NegativeTotal)
        );
        assert_eq!(
            Account::from_record(record(0, -1, Open, false)),
            Err(RecordError::NegativeTotal)
        );
        assert_eq!(
            Account::from_record(record(5, 0, Open, false)),
            Err(RecordError::NegativeBalance)
        );
        assert!(Account::from_record(record(5, 0, Open, true)).is_ok());
        assert_eq!(
            Account::from_record(record(0, 5, Closed, false)),
            Err(RecordError::ClosedWithBalance)
        );
        for (error, message) in [
            (
                RecordError::NegativeTotal,
                "stored account has a negative total",
            ),
            (
                RecordError::NegativeBalance,
                "stored account has a forbidden negative balance",
            ),
            (
                RecordError::ClosedWithBalance,
                "stored closed account has a balance",
            ),
        ] {
            assert_eq!(error.to_string(), message);
        }
        Ok(())
    }

    #[test]
    fn errors_have_messages() {
        assert_eq!(
            StatusError::Unchanged(Frozen).to_string(),
            "account is already frozen"
        );
        assert_eq!(
            StatusError::AccountClosed.to_string(),
            "a closed account cannot change status"
        );
        assert_eq!(
            StatusError::BalanceNotZero.to_string(),
            "an account can only close at zero balance"
        );
        let names: Vec<String> = AccountStatus::ALL.iter().map(|s| s.to_string()).collect();
        assert_eq!(names, ["open", "frozen", "closed"]);
    }
}

#[cfg(test)]
mod properties {
    use proptest::prelude::*;

    use super::*;

    /// The normal-balance table from docs/accounting/README.md, written out
    /// by hand so the code is checked against the documentation rather than
    /// against itself.
    const NORMAL_BALANCE_TABLE: [(AccountType, Side); 5] = [
        (AccountType::Asset, Side::Debit),
        (AccountType::Liability, Side::Credit),
        (AccountType::Equity, Side::Credit),
        (AccountType::Revenue, Side::Credit),
        (AccountType::Expense, Side::Debit),
    ];

    fn account_type() -> impl Strategy<Value = AccountType> {
        proptest::sample::select(AccountType::ALL.as_slice())
    }

    proptest! {
        #[test]
        fn balance_agrees_with_the_normal_balance_table(
            account_type in account_type(),
            debits in 0..=i64::MAX,
            credits in 0..=i64::MAX,
        ) {
            let increases_with = NORMAL_BALANCE_TABLE
                .iter()
                .find(|(t, _)| *t == account_type)
                .map(|(_, side)| *side);
            let expected = match increases_with {
                Some(Side::Debit) => debits - credits,
                Some(Side::Credit) => credits - debits,
                None => return Err(TestCaseError::fail("type missing from table")),
            };
            prop_assert_eq!(balance(account_type.normal_side(), debits, credits), Some(expected));
        }

        #[test]
        fn posting_on_the_normal_side_increases_the_balance(
            account_type in account_type(),
            start in 0..=i64::MAX / 2,
            amount in 1..=i64::MAX / 2,
        ) {
            let side = account_type.normal_side();
            let before = balance(side, start, start);
            let after = match side {
                Side::Debit => balance(side, start + amount, start),
                Side::Credit => balance(side, start, start + amount),
            };
            prop_assert_eq!(before.zip(after).map(|(b, a)| a - b), Some(amount));
        }

        #[test]
        fn closed_is_final(to in proptest::sample::select(AccountStatus::ALL.as_slice()), zero in any::<bool>()) {
            prop_assert!(AccountStatus::Closed.transition(to, zero).is_err());
        }
    }
}

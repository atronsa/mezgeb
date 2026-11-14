//! The [`Template`] trait and the one function that turns a template's
//! booking into an [`EntryDraft`].

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use mezgeb_core::{
    Account, AccountId, AccountsView, EntryDraft, LineDraft, Money, PositiveAmount, Side, Timestamp,
};

use crate::error::TemplateError;

/// The metadata key under which a draft records the template that booked
/// it, as `name.vN`. Reserved: a request may not set it.
pub const TEMPLATE_KEY: &str = "template";

/// Longest template name, in bytes.
const MAX_NAME: usize = 64;

/// A template name and version, written `name.vN`, such as
/// `p2p_transfer.v1`.
///
/// The name is 1 to 64 characters from `a–z 0–9 _`, starting with a letter.
/// The version is a number from 1, without leading zeros.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TemplateId {
    name: String,
    version: u16,
}

impl TemplateId {
    pub fn new(name: impl Into<String>, version: u16) -> Result<TemplateId, TemplateError> {
        let name = name.into();
        let valid = name.len() <= MAX_NAME
            && name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            && version >= 1;
        if valid {
            Ok(TemplateId { name, version })
        } else {
            Err(TemplateError::InvalidTemplateId(format!(
                "{name}.v{version}"
            )))
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn version(&self) -> u16 {
        self.version
    }
}

impl FromStr for TemplateId {
    type Err = TemplateError;

    fn from_str(s: &str) -> Result<TemplateId, TemplateError> {
        let invalid = || TemplateError::InvalidTemplateId(s.to_owned());
        let (name, version) = s.split_once(".v").ok_or_else(invalid)?;
        if version.starts_with('0') || !version.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let version: u16 = version.parse().map_err(|_| invalid())?;
        TemplateId::new(name, version).map_err(|_| invalid())
    }
}

impl fmt::Display for TemplateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.v{}", self.name, self.version)
    }
}

/// A named place in a template's booking, such as `payer_wallet`. The
/// request binds it to an account, which must be opened under one of the
/// chart account `codes` (TM-07): a role for a customer wallet can never be
/// bound to fee revenue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Role {
    pub name: &'static str,
    pub codes: &'static [u16],
    /// An optional role need only be bound when the booking uses it, such
    /// as fee revenue when there is a fee.
    pub optional: bool,
}

impl Role {
    /// A role the request must always bind.
    pub const fn new(name: &'static str, codes: &'static [u16]) -> Role {
        Role {
            name,
            codes,
            optional: false,
        }
    }

    /// A role the request binds only when the booking uses it.
    pub const fn optional(name: &'static str, codes: &'static [u16]) -> Role {
        Role {
            name,
            codes,
            optional: true,
        }
    }

    fn allows(&self, code: u16) -> bool {
        self.codes.contains(&code)
    }
}

/// One line of a template's booking: `amount` on `side` of whichever account
/// the request binds to `role`. A template never names an account ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leg {
    pub role: &'static str,
    pub side: Side,
    pub amount: PositiveAmount,
}

impl Leg {
    pub const fn debit(role: &'static str, amount: PositiveAmount) -> Leg {
        Leg {
            role,
            side: Side::Debit,
            amount,
        }
    }

    pub const fn credit(role: &'static str, amount: PositiveAmount) -> Leg {
        Leg {
            role,
            side: Side::Credit,
            amount,
        }
    }
}

/// A metadata key a template reads, such as `channel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataField {
    pub name: &'static str,
    /// Whether the request must set it.
    pub required: bool,
    /// The values it may take. Empty means any value.
    pub values: &'static [&'static str],
}

impl MetadataField {
    /// A key the request must set, to one of `values` (any value if empty).
    pub const fn required(name: &'static str, values: &'static [&'static str]) -> MetadataField {
        MetadataField {
            name,
            required: true,
            values,
        }
    }

    /// A key the request may set, to one of `values` (any value if empty).
    pub const fn optional(name: &'static str, values: &'static [&'static str]) -> MetadataField {
        MetadataField {
            name,
            required: false,
            values,
        }
    }
}

/// The amounts of an [`OperationRequest`], as a template reads them to build
/// its typed request. Every amount the template declares is present and
/// positive by the time it sees them, and no optional amount is negative.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    amounts: &'a BTreeMap<String, Money>,
}

impl Input<'_> {
    pub fn amount(&self, name: &str) -> Result<PositiveAmount, TemplateError> {
        let money = self
            .amounts
            .get(name)
            .ok_or_else(|| TemplateError::MissingAmount(name.to_owned()))?;
        PositiveAmount::new(*money).map_err(|_| TemplateError::AmountNotPositive(name.to_owned()))
    }

    /// An optional amount: `None` if it is absent or zero. A negative amount
    /// is refused.
    pub fn optional_amount(&self, name: &str) -> Result<Option<PositiveAmount>, TemplateError> {
        match self.amounts.get(name) {
            None => Ok(None),
            Some(money) if money.is_zero() => Ok(None),
            Some(_) => self.amount(name).map(Some),
        }
    }
}

/// The accounts a request binds to a template's roles, as the template sees
/// them while booking: by role, never by ID. Every role is bound, allowed and
/// distinct by the time the template sees them.
#[derive(Debug, Clone)]
pub struct Accounts<'a> {
    bound: Vec<(&'static str, AccountId, &'a Account)>,
}

impl Accounts<'_> {
    fn get(&self, role: &str) -> Result<&Account, TemplateError> {
        self.bound
            .iter()
            .find(|(name, ..)| *name == role)
            .map(|(.., account)| *account)
            .ok_or_else(|| TemplateError::UndeclaredRole(role.to_owned()))
    }

    /// The balance of the account bound to `role`, on its normal side.
    pub fn balance(&self, role: &str) -> Result<Money, TemplateError> {
        Ok(self.get(role)?.balance())
    }

    /// Whether the account bound to `role` has never been posted to.
    pub fn is_new(&self, role: &str) -> Result<bool, TemplateError> {
        let account = self.get(role)?;
        Ok(account.debits_total() == 0 && account.credits_total() == 0)
    }

    /// Refuses with [`TemplateError::AccountNotNew`] unless the account bound
    /// to `role` has never been posted to: an item that opens its own
    /// sub-account.
    pub fn require_new(&self, role: &str) -> Result<(), TemplateError> {
        if self.is_new(role)? {
            Ok(())
        } else {
            Err(TemplateError::AccountNotNew(role.to_owned()))
        }
    }

    /// Refuses with [`TemplateError::AmountNotBalance`] unless `amount` is
    /// the whole balance of the account bound to `role`: an item that moves
    /// whole.
    pub fn require_whole_balance(
        &self,
        role: &str,
        amount: PositiveAmount,
    ) -> Result<(), TemplateError> {
        if self.balance(role)? == amount.money() {
            Ok(())
        } else {
            Err(TemplateError::AmountNotBalance(role.to_owned()))
        }
    }
}

/// One template version: how one kind of business event is booked.
///
/// A released version never changes: entries name the version that booked
/// them, forever. A change to a booking is a new version, registered beside
/// the old one.
pub trait Template {
    /// The template's name, such as `"p2p_transfer"`.
    const NAME: &'static str;
    /// The version, from 1.
    const VERSION: u16;
    /// Every role the template books, each with the chart accounts it may be
    /// bound to. A request must bind exactly these roles.
    const ROLES: &'static [Role];
    /// Every amount the template reads. A request must give all of these,
    /// and no amount that is neither here nor in [`Template::OPTIONAL_AMOUNTS`].
    const AMOUNTS: &'static [&'static str];
    /// Amounts the request may leave out or set to zero, such as a fee.
    const OPTIONAL_AMOUNTS: &'static [&'static str] = &[];
    /// Metadata keys the template checks, such as a required `channel`.
    /// Other keys are copied into the entry unchecked.
    const METADATA: &'static [MetadataField] = &[];

    /// The typed request.
    type Request;

    /// Builds the typed request from a request's amounts.
    fn request(input: &Input<'_>) -> Result<Self::Request, TemplateError>;

    /// The booking: lines against roles, in the order they are posted. The
    /// template may read the bound accounts' balances to check its own
    /// rules, such as [`Accounts::require_whole_balance`].
    ///
    /// Those checks see the accounts as they were when the draft was made.
    /// The posting rules check the balances again when the engine posts the
    /// draft, with the accounts locked, so a check here is never the only
    /// guard against a concurrent posting.
    fn book(request: &Self::Request, accounts: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError>;
}

/// A request to book a business event with a template, by name: the form a
/// client sends, before it is typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationRequest {
    /// The template, as `name.vN`.
    pub template: String,
    /// The account bound to each of the template's roles.
    pub accounts: BTreeMap<String, AccountId>,
    /// Each amount the template reads, already computed by the client (the
    /// ledger never rounds, ADR-0002).
    pub amounts: BTreeMap<String, Money>,
    /// Client references, copied into the entry.
    pub metadata: BTreeMap<String, String>,
    pub event_time: Option<Timestamp>,
}

/// Books a typed request with template `T`: resolves each role to the
/// account the request binds to it, checks the bindings, and returns the
/// draft, with the template recorded under [`TEMPLATE_KEY`].
///
/// The draft still has to pass the posting rules, which the engine checks
/// when it posts it.
pub fn draft<T: Template, V: AccountsView + ?Sized>(
    request: &T::Request,
    accounts: &BTreeMap<String, AccountId>,
    mut metadata: BTreeMap<String, String>,
    event_time: Option<Timestamp>,
    view: &V,
) -> Result<EntryDraft, TemplateError> {
    let id = TemplateId::new(T::NAME, T::VERSION)?;

    for role in T::ROLES {
        if !role.optional && !accounts.contains_key(role.name) {
            return Err(TemplateError::MissingAccount(role.name.to_owned()));
        }
    }
    if let Some(extra) = accounts
        .keys()
        .find(|name| !T::ROLES.iter().any(|r| r.name == name.as_str()))
    {
        return Err(TemplateError::UnexpectedAccount(extra.clone()));
    }
    let mut bound: Vec<(&'static str, AccountId, &Account)> = Vec::with_capacity(T::ROLES.len());
    for role in T::ROLES {
        let Some(id) = accounts.get(role.name).copied() else {
            continue;
        };
        let account = view
            .account(id)
            .ok_or_else(|| TemplateError::AccountNotFound {
                role: role.name.to_owned(),
                account: id,
            })?;
        if !role.allows(account.code().value()) {
            return Err(TemplateError::AccountNotAllowed {
                role: role.name.to_owned(),
                account: id,
            });
        }
        bound.push((role.name, id, account));
    }
    for (i, (role, id, _)) in bound.iter().enumerate() {
        if let Some((other, ..)) = bound[..i].iter().find(|(_, other, _)| other == id) {
            return Err(TemplateError::SameAccount {
                role: (*role).to_owned(),
                other: (*other).to_owned(),
            });
        }
    }
    let accounts = Accounts { bound };
    let legs = T::book(request, &accounts)?;

    if metadata.contains_key(TEMPLATE_KEY) {
        return Err(TemplateError::ReservedMetadata);
    }
    for field in T::METADATA {
        match metadata.get(field.name) {
            None if field.required => {
                return Err(TemplateError::MissingMetadata(field.name.to_owned()));
            }
            Some(value) if !field.values.is_empty() && !field.values.contains(&value.as_str()) => {
                return Err(TemplateError::InvalidMetadata(field.name.to_owned()));
            }
            _ => {}
        }
    }

    let mut lines = Vec::with_capacity(legs.len());
    for leg in legs {
        let Some((_, account, _)) = accounts.bound.iter().find(|(role, ..)| *role == leg.role)
        else {
            // A declared optional role the booking needs but the request
            // left out, or a role the template never declared.
            return Err(if T::ROLES.iter().any(|r| r.name == leg.role) {
                TemplateError::MissingAccount(leg.role.to_owned())
            } else {
                TemplateError::UndeclaredRole(leg.role.to_owned())
            });
        };
        lines.push(LineDraft {
            account: *account,
            side: leg.side,
            amount: leg.amount.money(),
        });
    }

    metadata.insert(TEMPLATE_KEY.to_owned(), id.to_string());
    Ok(EntryDraft {
        lines,
        metadata,
        event_time,
        allow_duplicate_lines: false,
        refunds: None,
    })
}

/// Books an untyped request with template `T`: checks its amounts, builds
/// the typed request, then [`draft`]s it. The registry calls this after it
/// has matched `request.template` to `T`.
pub(crate) fn book_operation<T: Template>(
    request: &OperationRequest,
    view: &dyn AccountsView,
) -> Result<EntryDraft, TemplateError> {
    for name in T::AMOUNTS {
        if !request.amounts.contains_key(*name) {
            return Err(TemplateError::MissingAmount((*name).to_owned()));
        }
    }
    if let Some(extra) = request.amounts.keys().find(|name| {
        !T::AMOUNTS.contains(&name.as_str()) && !T::OPTIONAL_AMOUNTS.contains(&name.as_str())
    }) {
        return Err(TemplateError::UnexpectedAmount(extra.clone()));
    }
    let input = Input {
        amounts: &request.amounts,
    };
    for name in T::AMOUNTS {
        input.amount(name)?;
    }
    for name in T::OPTIONAL_AMOUNTS {
        input.optional_amount(name)?;
    }
    let typed = T::request(&input)?;
    draft::<T, _>(
        &typed,
        &request.accounts,
        request.metadata.clone(),
        request.event_time,
        view,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_ids_round_trip() -> Result<(), TemplateError> {
        for text in [
            "p2p_transfer.v1",
            "a.v1",
            "merchant_payment.v12",
            "x9_.v65535",
        ] {
            let id: TemplateId = text.parse()?;
            assert_eq!(id.to_string(), text);
        }
        let id: TemplateId = "p2p_transfer.v1".parse()?;
        assert_eq!((id.name(), id.version()), ("p2p_transfer", 1));
        Ok(())
    }

    #[test]
    fn malformed_template_ids_are_refused() {
        let long = format!("{}.v1", "a".repeat(MAX_NAME + 1));
        for text in [
            "",
            "p2p_transfer",
            "p2p_transfer.",
            "p2p_transfer.v",
            "p2p_transfer.v0",
            "p2p_transfer.v01",
            "p2p_transfer.v+1",
            "p2p_transfer.v65536",
            "p2p_transfer.v1.v2",
            "P2P_transfer.v1",
            "1p2p.v1",
            "_p2p.v1",
            "p2p-transfer.v1",
            ".v1",
            " p2p_transfer.v1",
            long.as_str(),
        ] {
            let result = text.parse::<TemplateId>();
            assert_eq!(
                result.map_err(|e| e.code()),
                Err(2001),
                "{text:?} should be refused"
            );
        }
        assert!(TemplateId::new("a".repeat(MAX_NAME), 1).is_ok());
        assert_eq!(TemplateId::new("ok", 0).map_err(|e| e.code()), Err(2001));
    }
}

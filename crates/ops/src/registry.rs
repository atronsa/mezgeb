//! Every template version, keyed by `name.vN`.

use std::collections::BTreeMap;
use std::fmt;

use mezgeb_core::{AccountsView, EntryDraft};

use crate::deposits::{DepositConfirmedV1, DepositUnmatchedV1, SuspenseResolveV1};
use crate::error::TemplateError;
use crate::p2p_transfer::P2pTransferV1;
use crate::payments::{MerchantPaymentV1, PaymentRequestSettleV1};
use crate::template::{
    MetadataField, OperationRequest, Role, Template, TemplateId, book_operation,
};
use crate::withdrawals::{
    WithdrawalConfirmedV1, WithdrawalFailedV1, WithdrawalRequestV1, WithdrawalSentV1,
};

/// Every template version this crate has released, in release order.
///
/// Append only. Entries name the version that booked them, so a released
/// version is never changed or removed; a test checks that
/// [`TemplateRegistry::standard`] registers exactly these.
pub const RELEASED: &[&str] = &[
    // Step 2.01.
    "p2p_transfer.v1",
    // Step 2.02.
    "deposit_confirmed.v1",
    "deposit_unmatched.v1",
    "suspense_resolve.v1",
    "withdrawal_request.v1",
    "withdrawal_sent.v1",
    "withdrawal_confirmed.v1",
    "withdrawal_failed.v1",
    // Step 2.03.
    "merchant_payment.v1",
    "payment_request_settle.v1",
];

type Book = fn(&OperationRequest, &dyn AccountsView) -> Result<EntryDraft, TemplateError>;

#[derive(Clone, Copy)]
struct Registered {
    roles: &'static [Role],
    amounts: &'static [&'static str],
    optional_amounts: &'static [&'static str],
    metadata: &'static [MetadataField],
    book: Book,
}

/// Why a template could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// This version is already registered.
    Duplicate(TemplateId),
    /// The template's name or version is not a valid [`TemplateId`].
    InvalidId(TemplateError),
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegisterError::Duplicate(id) => write!(f, "template {id} is already registered"),
            RegisterError::InvalidId(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RegisterError {}

/// The templates a ledger accepts, keyed by `name.vN`.
#[derive(Clone, Default)]
pub struct TemplateRegistry {
    templates: BTreeMap<TemplateId, Registered>,
}

impl fmt::Debug for TemplateRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.templates.keys()).finish()
    }
}

impl TemplateRegistry {
    /// A registry with no templates.
    pub fn new() -> TemplateRegistry {
        TemplateRegistry::default()
    }

    /// A registry with every released template ([`RELEASED`]).
    pub fn standard() -> TemplateRegistry {
        let mut registry = TemplateRegistry::new();
        let builtin = [
            TemplateRegistry::register::<P2pTransferV1>,
            TemplateRegistry::register::<DepositConfirmedV1>,
            TemplateRegistry::register::<DepositUnmatchedV1>,
            TemplateRegistry::register::<SuspenseResolveV1>,
            TemplateRegistry::register::<WithdrawalRequestV1>,
            TemplateRegistry::register::<WithdrawalSentV1>,
            TemplateRegistry::register::<WithdrawalConfirmedV1>,
            TemplateRegistry::register::<WithdrawalFailedV1>,
            TemplateRegistry::register::<MerchantPaymentV1>,
            TemplateRegistry::register::<PaymentRequestSettleV1>,
        ];
        for register in builtin {
            register(&mut registry)
                .unwrap_or_else(|e| unreachable!("built-in templates are distinct: {e}"));
        }
        registry
    }

    /// Adds template `T`. Refuses a version that is already registered, and
    /// a [`Template::NAME`] or [`Template::VERSION`] that is not a valid
    /// [`TemplateId`].
    pub fn register<T: Template>(&mut self) -> Result<(), RegisterError> {
        let id = TemplateId::new(T::NAME, T::VERSION).map_err(RegisterError::InvalidId)?;
        if self.templates.contains_key(&id) {
            return Err(RegisterError::Duplicate(id));
        }
        self.templates.insert(
            id,
            Registered {
                roles: T::ROLES,
                amounts: T::AMOUNTS,
                optional_amounts: T::OPTIONAL_AMOUNTS,
                metadata: T::METADATA,
                book: book_operation::<T>,
            },
        );
        Ok(())
    }

    /// Every registered template version, in order.
    pub fn ids(&self) -> impl Iterator<Item = &TemplateId> {
        self.templates.keys()
    }

    /// The roles a template version books.
    pub fn roles(&self, template: &str) -> Result<&'static [Role], TemplateError> {
        Ok(self.find(template)?.roles)
    }

    /// The amounts a template version reads.
    pub fn amounts(&self, template: &str) -> Result<&'static [&'static str], TemplateError> {
        Ok(self.find(template)?.amounts)
    }

    /// The amounts a template version reads if they are given.
    pub fn optional_amounts(
        &self,
        template: &str,
    ) -> Result<&'static [&'static str], TemplateError> {
        Ok(self.find(template)?.optional_amounts)
    }

    /// The metadata keys a template version checks.
    pub fn metadata(&self, template: &str) -> Result<&'static [MetadataField], TemplateError> {
        Ok(self.find(template)?.metadata)
    }

    /// Books `request` with the template version it names.
    pub fn draft(
        &self,
        request: &OperationRequest,
        view: &dyn AccountsView,
    ) -> Result<EntryDraft, TemplateError> {
        (self.find(&request.template)?.book)(request, view)
    }

    fn find(&self, template: &str) -> Result<&Registered, TemplateError> {
        let id: TemplateId = template.parse()?;
        match self.templates.get(&id) {
            Some(registered) => Ok(registered),
            None if self.templates.keys().any(|k| k.name() == id.name()) => {
                Err(TemplateError::UnknownVersion(template.to_owned()))
            }
            None => Err(TemplateError::UnknownTemplate(template.to_owned())),
        }
    }
}

//! Posting templates (ADR-0002, layer 2, step 2.01).
//!
//! Named, versioned functions such as `p2p_transfer.v1` that turn a typed
//! request into an [`EntryDraft`](mezgeb_core::EntryDraft). They define
//! *how* a business event is booked, never *whether* it is allowed: that is
//! the client application's decision.
//!
//! - [`Template`]: one template version. It books [`Leg`]s against named
//!   [`Role`]s such as `payer_wallet`, never against account IDs. The
//!   request binds each role to an account, and each role fixes which chart
//!   accounts it may be bound to (TM-07).
//! - [`TemplateRegistry`]: every template version, keyed by `name.vN`. A
//!   version, once released, is never changed or removed, because the
//!   entries it booked name it forever.
//! - [`TemplateError`]: why a request could not be booked, with stable
//!   numeric codes in the 2000 range.
//!
//! The draft records its template in the entry metadata under
//! [`TEMPLATE_KEY`]. The engine stores the metadata and includes it in the
//! idempotency fingerprint, but knows nothing about templates: a new
//! template is added here, without touching the engine.
//!
//! Each template maps to a worked entry in `docs/accounting/entries/` and is
//! tested against it.

mod error;
mod p2p_transfer;
mod registry;
mod template;

pub use error::TemplateError;
pub use p2p_transfer::{P2pTransferRequest, P2pTransferV1};
pub use registry::{RELEASED, RegisterError, TemplateRegistry};
pub use template::{Input, Leg, OperationRequest, Role, TEMPLATE_KEY, Template, TemplateId, draft};

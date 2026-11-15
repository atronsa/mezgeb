//! Authorization before anything else (ADR-0017). Each handler names the
//! scopes its operation needs as the type of its first extractor,
//! `Auth<S>`. The caller is authenticated and its scopes checked before the
//! path, the query or the body is read, so a caller without the scope gets
//! `403 forbidden` whatever it sent. A handler without an `Auth<S>` would
//! answer anyone; the authorization tests call every operation of the
//! contract without a token to prove there is none.

use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use super::Principal;
use crate::AppState;
use crate::http::extract::Caller;
use crate::problem::ApiError;

/// Scopes that widen what other scopes reach instead of naming an
/// operation: `accounts:read_all` reads every client's accounts and entries.
pub const MODIFIERS: &[&str] = &["accounts:read_all"];

/// The scopes an operation accepts: the caller needs at least one.
pub trait Scope {
    const NAMES: &'static [&'static str];
}

macro_rules! scopes {
    ($($(#[$doc:meta])* $ty:ident => [$($name:literal),+],)*) => {
        $(
            $(#[$doc])*
            pub struct $ty;

            impl Scope for $ty {
                const NAMES: &'static [&'static str] = &[$($name),+];
            }
        )*

        /// Every scope an operation can need, as the contract lists them.
        pub const ALL: &[&[&str]] = &[$(&[$($name),+]),*];
    };
}

scopes! {
    AccountsRead => ["accounts:read"],
    AccountsWrite => ["accounts:write"],
    AccountsAdmin => ["accounts:admin"],
    OperationsWrite => ["operations:write"],
    EntriesRead => ["entries:read"],
    CorrectionsWrite => ["corrections:write"],
    AdjustmentsRequest => ["adjustments:request"],
    /// Reading an adjustment request: the maker, or the checker deciding it.
    AdjustmentsView => ["adjustments:request", "adjustments:approve"],
    AdjustmentsApprove => ["adjustments:approve"],
    HoldsRead => ["holds:read"],
    HoldsWrite => ["holds:write"],
    PeriodsRead => ["periods:read"],
    PeriodsClose => ["periods:close"],
    ReportsRead => ["reports:read"],
    OfflineWrite => ["offline:write"],
}

/// An authenticated caller holding one of the scopes of `S`.
pub struct Auth<S>(pub Principal, pub(crate) PhantomData<S>);

impl<S: Scope> FromRequestParts<AppState> for Auth<S> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Auth<S>, ApiError> {
        let Caller(principal) = Caller::from_request_parts(parts, state).await?;
        if S::NAMES.iter().any(|scope| principal.has(scope)) {
            // For the checks that follow, such as the templates the caller
            // may post.
            parts.extensions.insert(principal.clone());
            Ok(Auth(principal, PhantomData))
        } else {
            Err(ApiError::forbidden(&S::NAMES.join(" or ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scope_is_listed_once() {
        let mut names: Vec<&str> = ALL.iter().flat_map(|s| s.iter().copied()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 14, "{names:?}");
        assert_eq!(
            AdjustmentsView::NAMES,
            ["adjustments:request", "adjustments:approve"]
        );
    }
}

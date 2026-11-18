//! Who is calling, and what they may do (step 2.08, ADR-0017).
//!
//! A caller sends `Authorization: Bearer <token>`, a token it got from
//! `POST /oauth/token` ([`oauth`]) with its credential's ID and secret. The
//! token is looked up by its SHA-256 on every request. It is valid only
//! while it has not expired and its credential and its client are active,
//! so revoking either ends it at once.
//!
//! The caller's scopes are the token's; the templates it may post come from
//! its credential's roles in the permission matrix ([`permissions`]). Each
//! operation checks its scope before the request is read ([`scope`]). Each
//! client's requests are rate-limited ([`rate`]): over its limit, a request
//! is refused with `429 rate_limited` before anything else. A caller whose
//! address has had too many tokens refused is refused before the lookup, if
//! the operator turned that limit on (ADR-0021).

use std::collections::BTreeSet;
use std::net::IpAddr;

use mezgeb_core::{ActorId, ClientId};
use uuid::Uuid;

use crate::AppState;
use crate::problem::ApiError;

pub mod oauth;
pub mod permissions;
pub mod rate;
pub mod scope;

pub use permissions::Permissions;

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub client: ClientId,
    /// The credential the token was issued to.
    pub credential: Uuid,
    /// The person or system acting, recorded on corrections.
    pub actor: ActorId,
    pub scopes: BTreeSet<String>,
    /// The templates the caller may post.
    pub templates: BTreeSet<String>,
    /// The client's rate limit: requests per second, and the burst.
    pub rate_per_second: u32,
    pub burst: u32,
}

impl Principal {
    /// Refuses with `403 forbidden` unless the caller has `scope`.
    pub fn require(&self, scope: &str) -> Result<(), ApiError> {
        if self.has(scope) {
            Ok(())
        } else {
            Err(ApiError::forbidden(scope))
        }
    }

    pub fn has(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }

    /// Whether the caller reads every client's accounts and entries, not
    /// only its own.
    pub fn sees_all(&self) -> bool {
        self.has("accounts:read_all")
    }
}

/// The caller of a request with this `Authorization` header value, from
/// `address`. The client's rate limit is checked by [`admit`], once the
/// caller is known to the logs.
pub async fn authenticate(
    state: &AppState,
    header: Option<&str>,
    address: Option<IpAddr>,
) -> Result<Principal, ApiError> {
    let token = header
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(ApiError::unauthenticated)?;
    if !state.limits.reserve_address(address) {
        return Err(ApiError::new(
            "rate_limited",
            "Too many refused requests from this address; retry after Retry-After seconds.",
        ));
    }
    let digest = mezgeb_engine::token_digest(token);
    let mut conn = state.pool.acquire().await.map_err(|e| match e {
        sqlx::Error::PoolTimedOut => {
            ApiError::new("busy", "No database connection was free in time.")
        }
        other => ApiError::internal(other),
    })?;
    let owner = mezgeb_engine::token_owner(&mut conn, &digest).await?;
    drop(conn);
    let Some(owner) = owner else {
        // The reservation stays spent: this was a failure.
        return Err(ApiError::unauthenticated());
    };
    state.limits.release_address(address);
    Ok(Principal {
        client: owner.client,
        credential: owner.credential,
        actor: owner.actor,
        templates: state.permissions.templates(&owner.roles),
        scopes: owner.scopes.into_iter().collect(),
        rate_per_second: owner.rate_per_second,
        burst: owner.burst,
    })
}

/// Takes one request from the caller's client's rate limit: `429` if it is
/// spent.
pub fn admit(state: &AppState, principal: &Principal) -> Result<(), ApiError> {
    if state
        .limits
        .clients
        .take(principal.client, principal.rate_per_second, principal.burst)
    {
        Ok(())
    } else {
        Err(ApiError::new(
            "rate_limited",
            "This client has sent more requests than its limit; retry after Retry-After seconds.",
        ))
    }
}

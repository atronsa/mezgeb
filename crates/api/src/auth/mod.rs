//! Who is calling, and what they may do.
//!
//! **Interim, until step 2.08.** The contract describes OAuth 2.0 client
//! credentials. Until step 2.08 builds that, the service reads bearer tokens
//! from a file of their SHA-256 hashes (never the tokens themselves):
//!
//! ```toml
//! [[tokens]]
//! sha256 = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
//! client_id = "0192f3c4-0000-7000-8000-00000000c11e"
//! actor = "app:wallet"
//! scopes = ["accounts:read", "accounts:write", "operations:write", "entries:read"]
//! ```
//!
//! Every route names the scope it needs, and a token without it is refused:
//! default deny. What the interim scheme does not do yet, and step 2.08 must:
//! expiring tokens, a clients table, and limiting a client to its own
//! accounts and entries.

use mezgeb_core::{ActorId, ClientId};

use crate::problem::ApiError;

pub mod scope;
mod tokens;

pub use tokens::{Tokens, token_hash};

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub client: ClientId,
    /// The person or system acting, recorded on corrections.
    pub actor: ActorId,
    pub scopes: Vec<String>,
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
        self.scopes.iter().any(|s| s == scope)
    }
}

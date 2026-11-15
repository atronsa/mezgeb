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

use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;

use mezgeb_core::{ActorId, ClientId};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::problem::ApiError;

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenFile {
    tokens: Vec<TokenEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenEntry {
    sha256: String,
    client_id: String,
    actor: String,
    scopes: Vec<String>,
}

/// The known tokens, by the hex SHA-256 of the token.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    by_hash: HashMap<String, Principal>,
}

/// The hex SHA-256 of a token, as the token file stores it.
pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

impl Tokens {
    /// Reads a token file.
    pub fn load(path: &Path) -> Result<Tokens, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Tokens::parse(&text)
    }

    /// Parses the text of a token file.
    pub fn parse(text: &str) -> Result<Tokens, String> {
        let file: TokenFile = toml::from_str(text).map_err(|e| format!("token file: {e}"))?;
        let mut by_hash = HashMap::new();
        for entry in file.tokens {
            let hash = entry.sha256.to_ascii_lowercase();
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!(
                    "token file: {:?} is not a SHA-256 in hex",
                    entry.sha256
                ));
            }
            let principal = Principal {
                client: entry
                    .client_id
                    .parse()
                    .map_err(|_| format!("token file: bad client_id {:?}", entry.client_id))?,
                actor: ActorId::new(&entry.actor)
                    .map_err(|e| format!("token file: actor {:?}: {e}", entry.actor))?,
                scopes: entry.scopes,
            };
            if by_hash.insert(hash, principal).is_some() {
                return Err("token file: the same token is listed twice".to_owned());
            }
        }
        Ok(Tokens { by_hash })
    }

    /// The caller of a request with this `Authorization` header value.
    pub fn authenticate(&self, header: Option<&str>) -> Result<Principal, ApiError> {
        let token = header
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(ApiError::unauthenticated)?;
        self.by_hash
            .get(&token_hash(token))
            .cloned()
            .ok_or_else(ApiError::unauthenticated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
[[tokens]]
sha256 = "9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08"
client_id = "0192f3c4-0000-7000-8000-00000000c11e"
actor = "app:wallet"
scopes = ["operations:write"]
"#;

    #[test]
    fn tokens_are_matched_by_their_hash() -> Result<(), String> {
        // sha256("test")
        let tokens = Tokens::parse(FILE)?;
        let principal = tokens
            .authenticate(Some("Bearer test"))
            .map_err(|e| e.code().to_owned())?;
        assert_eq!(principal.actor.as_str(), "app:wallet");
        assert!(principal.has("operations:write"));
        assert!(principal.require("periods:close").is_err());
        for header in [
            None,
            Some("test"),
            Some("Bearer "),
            Some("Bearer wrong"),
            Some("Basic dGVzdA=="),
        ] {
            assert_eq!(
                tokens.authenticate(header).map_err(|e| e.code().to_owned()),
                Err("unauthenticated".to_owned()),
                "{header:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn bad_token_files_are_refused() {
        assert!(
            Tokens::parse(
                "[[tokens]]\nsha256 = \"abc\"\nclient_id = \"x\"\nactor = \"a\"\nscopes = []\n"
            )
            .is_err()
        );
        assert!(Tokens::parse(&format!("{FILE}{FILE}")).is_err());
        assert!(Tokens::parse("tokens = 1").is_err());
    }
}

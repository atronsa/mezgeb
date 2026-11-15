//! The ledger's signing key (ADR-0018): Ed25519, for receipts and
//! checkpoints.
//!
//! The key is a 32-byte seed in hex, read from a file the operator controls
//! (`MEZGEB_SIGNING_KEY_FILE`); step 3.12 moves it to a key service. It is
//! never stored in the database: only its public half is, in
//! `signing_keys`, under a key ID (the first 8 bytes of the SHA-256 of the
//! public key, in hex), so every signature names the key that made it and
//! old signatures stay verifiable after a key is retired.

use std::fmt;
use std::path::Path;

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use mezgeb_core::LedgerError;
use mezgeb_core::chain::hex;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

use crate::rows::{db, storage};

/// A signing key and its ID.
#[derive(Clone)]
pub struct Signer {
    key: SigningKey,
    key_id: String,
}

impl fmt::Debug for Signer {
    // Never print the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signer")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

/// The ID of a public key: the first 8 bytes of its SHA-256, in hex.
pub fn key_id(public: &[u8; 32]) -> String {
    hex(&Sha256::digest(public)[..8])
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

impl Signer {
    /// A signer from a 32-byte seed.
    pub fn from_seed(seed: [u8; 32]) -> Signer {
        let key = SigningKey::from_bytes(&seed);
        let key_id = key_id(&key.verifying_key().to_bytes());
        Signer { key, key_id }
    }

    /// A signer from a seed in hex (64 digits).
    pub fn from_hex(text: &str) -> Result<Signer, String> {
        let bytes = decode_hex(text).ok_or("the signing key is not hex")?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "the signing key must be 32 bytes (64 hex digits)")?;
        Ok(Signer::from_seed(seed))
    }

    /// A signer from a file holding the seed in hex.
    pub fn from_file(path: &Path) -> Result<Signer, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read the signing key {}: {e}", path.display()))?;
        Signer::from_hex(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A new random key's seed, in hex, for `mezgeb-cli key generate`.
    pub fn generate_hex() -> Result<String, LedgerError> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| storage(format!("no randomness: {e}")))?;
        Ok(hex(&seed))
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }

    /// Records the public key, once, so its signatures can be verified.
    pub async fn register(&self, conn: &mut PgConnection) -> Result<(), LedgerError> {
        sqlx::query(
            "INSERT INTO signing_keys (key_id, public_key) VALUES ($1, $2)
             ON CONFLICT (key_id) DO NOTHING",
        )
        .bind(&self.key_id)
        .bind(&self.public_key()[..])
        .execute(&mut *conn)
        .await
        .map_err(db)?;
        Ok(())
    }
}

/// Whether `signature` is a valid signature of `message` by `public`.
pub fn verify_signature(public: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok(public) = <[u8; 32]>::try_from(public) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    key.verify_strict(message, &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_verify_with_the_public_key_only() -> Result<(), String> {
        let signer = Signer::from_hex(&"07".repeat(32))?;
        assert_eq!(signer.key_id().len(), 16);
        let signature = signer.sign(b"entry 1");
        assert!(verify_signature(
            &signer.public_key(),
            b"entry 1",
            &signature
        ));
        assert!(!verify_signature(
            &signer.public_key(),
            b"entry 2",
            &signature
        ));
        let other = Signer::from_hex(&"08".repeat(32))?;
        assert!(!verify_signature(
            &other.public_key(),
            b"entry 1",
            &signature
        ));
        assert_ne!(other.key_id(), signer.key_id());
        assert!(
            !format!("{signer:?}").contains("07"),
            "the key is never printed"
        );
        for bad in ["", "zz", &"07".repeat(31), &"07".repeat(33)] {
            assert!(Signer::from_hex(bad).is_err(), "{bad:?}");
        }
        let generated = Signer::generate_hex().map_err(|e| e.to_string())?;
        assert!(Signer::from_hex(&generated).is_ok());
        Ok(())
    }
}

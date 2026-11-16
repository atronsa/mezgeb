//! Offline signatures (protocol §2): ECDSA over P-256 with SHA-256.
//!
//! - A signature is 64 bytes, `r ‖ s`, with `s` at most `n/2` (low-S). A
//!   verifier refuses any other, so a valid signature cannot be altered into
//!   a second valid one.
//! - A public key is a 33-byte SEC1 compressed point.
//! - A key ID is the first 8 bytes of the SHA-256 of the compressed key, in
//!   hex.
//!
//! The ledger's own receipts and checkpoints are Ed25519 (ADR-0018); this
//! module is only for the offline protocol, whose keys live in card chips
//! and phone keystores (ADR-0019).

use std::fmt;

use mezgeb_core::chain::hex;
use p256::ecdsa::signature::{Signer as _, Verifier as _};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// Length of a compressed public key.
pub const PUBLIC_KEY_LEN: usize = 33;
/// Length of a signature, `r ‖ s`.
pub const SIGNATURE_LEN: usize = 64;

/// Why a key or signature was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// Not a 33-byte compressed point on P-256.
    BadPublicKey,
    /// Not a valid P-256 secret scalar.
    BadSecretKey,
    /// Not 64 bytes, not low-S, or not by this key over this body.
    BadSignature,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CryptoError::BadPublicKey => "not a compressed P-256 public key",
            CryptoError::BadSecretKey => "not a P-256 secret key",
            CryptoError::BadSignature => "the signature does not verify",
        })
    }
}

impl std::error::Error for CryptoError {}

/// A P-256 public key, held in its compressed form.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey {
    key: VerifyingKey,
    compressed: [u8; PUBLIC_KEY_LEN],
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.key_id())
    }
}

impl PublicKey {
    /// Accepts the 33-byte compressed form only, so each key has one
    /// spelling and one key ID.
    pub fn from_compressed(bytes: &[u8]) -> Result<PublicKey, CryptoError> {
        let compressed: [u8; PUBLIC_KEY_LEN] =
            bytes.try_into().map_err(|_| CryptoError::BadPublicKey)?;
        if !matches!(compressed[0], 2 | 3) {
            return Err(CryptoError::BadPublicKey);
        }
        let key =
            VerifyingKey::from_sec1_bytes(&compressed).map_err(|_| CryptoError::BadPublicKey)?;
        Ok(PublicKey { key, compressed })
    }

    fn from_verifying(key: VerifyingKey) -> PublicKey {
        let point = key.to_sec1_point(true);
        let mut compressed = [0; PUBLIC_KEY_LEN];
        compressed.copy_from_slice(point.as_bytes());
        PublicKey { key, compressed }
    }

    pub fn compressed(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.compressed
    }

    /// The first 8 bytes of the SHA-256 of the compressed key, in hex.
    pub fn key_id(&self) -> String {
        hex(&Sha256::digest(self.compressed)[..8])
    }

    /// Checks `signature` over `body`: 64 bytes, low-S, by this key.
    pub fn verify(&self, body: &[u8], signature: &[u8]) -> Result<(), CryptoError> {
        let signature = Signature::from_slice(signature).map_err(|_| CryptoError::BadSignature)?;
        if signature.normalize_s() != signature {
            return Err(CryptoError::BadSignature);
        }
        self.key
            .verify(body, &signature)
            .map_err(|_| CryptoError::BadSignature)
    }
}

/// A P-256 signing key: the ledger's issuer key, or a test device's.
///
/// Signatures are deterministic (RFC 6979) and low-S, so the same key and
/// body always give the same 64 bytes, which the test vectors rely on.
#[derive(Clone)]
pub struct SecretKey {
    key: SigningKey,
    public: PublicKey,
}

impl fmt::Debug for SecretKey {
    // Never print the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKey")
            .field("key_id", &self.public.key_id())
            .finish_non_exhaustive()
    }
}

impl SecretKey {
    /// A key from its 32-byte big-endian scalar, which must be in `1..n`.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<SecretKey, CryptoError> {
        let key = SigningKey::from_slice(bytes).map_err(|_| CryptoError::BadSecretKey)?;
        let public = PublicKey::from_verifying(*key.verifying_key());
        Ok(SecretKey { key, public })
    }

    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    /// Signs `body`: `r ‖ s`, low-S.
    pub fn sign(&self, body: &[u8]) -> [u8; SIGNATURE_LEN] {
        let signature: Signature = self.key.sign(body);
        let signature = signature.normalize_s();
        let mut out = [0; SIGNATURE_LEN];
        out.copy_from_slice(&signature.to_bytes());
        out
    }
}

/// The other valid-looking form of a signature: `s` replaced by `n − s`.
/// ECDSA accepts both; the protocol accepts only the low one. For tests and
/// test vectors.
pub fn high_s(signature: &[u8; SIGNATURE_LEN]) -> Option<[u8; SIGNATURE_LEN]> {
    let (r, s) = Signature::from_slice(signature).ok()?.split_scalars();
    let flipped = Signature::from_scalars(r.to_bytes(), (-*s).to_bytes()).ok()?;
    let mut out = [0; SIGNATURE_LEN];
    out.copy_from_slice(&flipped.to_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> SecretKey {
        let mut bytes = [0; 32];
        bytes[31] = n;
        SecretKey::from_bytes(&bytes).expect("a valid scalar")
    }

    #[test]
    fn a_signature_verifies_and_one_bit_off_does_not() {
        let k = key(7);
        let body = b"mezgeb.offline.voucher.v1\0hello";
        let sig = k.sign(body);
        assert_eq!(k.public_key().verify(body, &sig), Ok(()));

        for byte in 0..body.len() {
            let mut tampered = body.to_vec();
            tampered[byte] ^= 1;
            assert!(k.public_key().verify(&tampered, &sig).is_err());
        }
        for byte in 0..SIGNATURE_LEN {
            let mut tampered = sig;
            tampered[byte] ^= 1;
            assert!(k.public_key().verify(body, &tampered).is_err());
        }
        assert!(key(8).public_key().verify(body, &sig).is_err());
    }

    #[test]
    fn signatures_are_deterministic_and_low_s() {
        let k = key(9);
        assert_eq!(k.sign(b"x"), k.sign(b"x"));
        let sig = k.sign(b"x");
        let high = high_s(&sig).expect("a high-S form");
        assert_ne!(high, sig);
        assert_eq!(
            k.public_key().verify(b"x", &high),
            Err(CryptoError::BadSignature)
        );
    }

    #[test]
    fn only_compressed_keys_are_accepted() {
        let k = key(5);
        let compressed = k.public_key().compressed();
        assert_eq!(
            PublicKey::from_compressed(compressed).map(|p| p.key_id()),
            Ok(k.public_key().key_id())
        );
        let uncompressed = k.public_key().key.to_sec1_point(false);
        assert_eq!(
            PublicKey::from_compressed(uncompressed.as_bytes()),
            Err(CryptoError::BadPublicKey)
        );
        assert_eq!(
            // x = 2^256 − 1 is not below the field prime.
            PublicKey::from_compressed(&[[2].as_slice(), &[0xff; 32]].concat()).err(),
            Some(CryptoError::BadPublicKey)
        );
        assert_eq!(key(1).public_key().key_id().len(), 16);
    }

    #[test]
    fn zero_is_not_a_secret_key() {
        assert_eq!(
            SecretKey::from_bytes(&[0; 32]).err(),
            Some(CryptoError::BadSecretKey)
        );
    }
}

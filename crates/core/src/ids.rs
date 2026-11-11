//! Typed identifiers (ADR-0006).
//!
//! Each kind of ID is its own type, so an account ID can never be passed
//! where an entry ID is expected. Ledger IDs are UUIDv7. They are generated
//! by the engine, which has a clock; this crate only validates them, so it
//! stays free of clocks and randomness.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant};

/// Errors from parsing or validating an ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    /// Not a lowercase, hyphenated UUID such as
    /// `017f22e2-79b0-7cc3-98c4-dc0c0c07398f`.
    Malformed,
    /// A valid UUID, but not version 7 (the nil UUID included).
    NotVersion7,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::Malformed => f.write_str("id is not a lowercase hyphenated UUID"),
            IdError::NotVersion7 => f.write_str("id is not a UUIDv7"),
        }
    }
}

impl std::error::Error for IdError {}

fn require_v7(uuid: Uuid) -> Result<Uuid, IdError> {
    if uuid.get_version_num() == 7 && uuid.get_variant() == Variant::RFC4122 {
        Ok(uuid)
    } else {
        Err(IdError::NotVersion7)
    }
}

/// Parses the canonical text form only, so each ID has exactly one spelling.
fn parse_canonical(s: &str) -> Result<Uuid, IdError> {
    let uuid = Uuid::try_parse(s).map_err(|_| IdError::Malformed)?;
    let mut buf = Uuid::encode_buffer();
    if uuid.hyphenated().encode_lower(&mut buf) == s {
        require_v7(uuid)
    } else {
        Err(IdError::Malformed)
    }
}

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        ///
        /// Always a UUIDv7. Serialised as its lowercase hyphenated string.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(Uuid);

        impl $name {
            /// Accepts a UUID only if it is version 7.
            pub fn from_uuid(uuid: Uuid) -> Result<Self, IdError> {
                require_v7(uuid).map(Self)
            }

            /// The underlying UUID, for storage.
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, IdError> {
                parse_canonical(s).map(Self)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(s: String) -> Result<Self, IdError> {
                s.parse()
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.to_string()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }
    };
}

uuid_id! {
    /// Identifies an account.
    AccountId
}

uuid_id! {
    /// Identifies a journal entry.
    EntryId
}

uuid_id! {
    /// Identifies a hold (ADR-0004).
    HoldId
}

uuid_id! {
    /// Identifies a client application that calls the ledger.
    ClientId
}

/// Longest accepted idempotency key, in characters.
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;

/// A client-chosen key that makes a write happen at most once (ADR-0008,
/// INV-13). Unique per client.
///
/// 1 to 128 characters from `A–Z a–z 0–9 - _ . :`, so a UUID, a ULID or
/// `order:2026-11-10:42` all work, and the key is safe in logs and headers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IdempotencyKey(String);

/// Errors from validating an idempotency key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyKeyError {
    Empty,
    TooLong,
    InvalidCharacter,
}

impl fmt::Display for IdempotencyKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdempotencyKeyError::Empty => f.write_str("idempotency key is empty"),
            IdempotencyKeyError::TooLong => write!(
                f,
                "idempotency key is longer than {MAX_IDEMPOTENCY_KEY_LEN} characters"
            ),
            IdempotencyKeyError::InvalidCharacter => {
                f.write_str("idempotency key has an invalid character")
            }
        }
    }
}

impl std::error::Error for IdempotencyKeyError {}

impl IdempotencyKey {
    pub fn new(key: impl Into<String>) -> Result<IdempotencyKey, IdempotencyKeyError> {
        let key = key.into();
        if key.is_empty() {
            Err(IdempotencyKeyError::Empty)
        } else if key.len() > MAX_IDEMPOTENCY_KEY_LEN {
            Err(IdempotencyKeyError::TooLong)
        } else if !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
        {
            Err(IdempotencyKeyError::InvalidCharacter)
        } else {
            Ok(IdempotencyKey(key))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for IdempotencyKey {
    type Err = IdempotencyKeyError;

    fn from_str(s: &str) -> Result<Self, IdempotencyKeyError> {
        IdempotencyKey::new(s)
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = IdempotencyKeyError;

    fn try_from(s: String) -> Result<Self, IdempotencyKeyError> {
        IdempotencyKey::new(s)
    }
}

impl From<IdempotencyKey> for String {
    fn from(key: IdempotencyKey) -> String {
        key.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UUIDv7 example from RFC 9562.
    const V7: &str = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f";
    const V4: &str = "f47ac10b-58cc-4372-a567-0e02b2c3d479";

    #[test]
    fn accepts_canonical_v7() -> Result<(), IdError> {
        let id: AccountId = V7.parse()?;
        assert_eq!(id.to_string(), V7);
        assert_eq!(AccountId::from_uuid(id.as_uuid()), Ok(id));
        assert_eq!(String::from(id), V7);
        Ok(())
    }

    #[test]
    fn rejects_other_versions_and_spellings() {
        assert_eq!(V4.parse::<EntryId>(), Err(IdError::NotVersion7));
        assert_eq!(
            "00000000-0000-0000-0000-000000000000".parse::<EntryId>(),
            Err(IdError::NotVersion7)
        );
        // Version 7 but not the RFC variant.
        assert_eq!(
            "017f22e2-79b0-7cc3-08c4-dc0c0c07398f".parse::<EntryId>(),
            Err(IdError::NotVersion7)
        );
        for bad in [
            "",
            "not-a-uuid",
            "017F22E2-79B0-7CC3-98C4-DC0C0C07398F",
            "017f22e279b07cc398c4dc0c0c07398f",
            "{017f22e2-79b0-7cc3-98c4-dc0c0c07398f}",
            "urn:uuid:017f22e2-79b0-7cc3-98c4-dc0c0c07398f",
        ] {
            assert_eq!(bad.parse::<HoldId>(), Err(IdError::Malformed), "{bad}");
        }
        assert_eq!(ClientId::from_uuid(Uuid::nil()), Err(IdError::NotVersion7));
    }

    #[test]
    fn id_errors_have_messages() {
        assert_eq!(
            IdError::Malformed.to_string(),
            "id is not a lowercase hyphenated UUID"
        );
        assert_eq!(IdError::NotVersion7.to_string(), "id is not a UUIDv7");
    }

    #[test]
    fn ids_serialize_as_strings() -> Result<(), serde_json::Error> {
        let json = format!("\"{V7}\"");
        let id: ClientId = serde_json::from_str(&json)?;
        assert_eq!(serde_json::to_string(&id)?, json);
        assert!(serde_json::from_str::<ClientId>(&format!("\"{V4}\"")).is_err());
        Ok(())
    }

    #[test]
    fn idempotency_key_rules() {
        for good in ["a", "order:2026-11-10:42", V7, "A.b_c-d", &"k".repeat(128)] {
            let key = IdempotencyKey::new(good);
            assert_eq!(key.as_ref().map(IdempotencyKey::as_str), Ok(good));
        }
        assert_eq!(IdempotencyKey::new(""), Err(IdempotencyKeyError::Empty));
        assert_eq!(
            IdempotencyKey::new("k".repeat(129)),
            Err(IdempotencyKeyError::TooLong)
        );
        for bad in ["a b", "a/b", "ключ", "a\n", "a+b"] {
            assert_eq!(
                IdempotencyKey::new(bad),
                Err(IdempotencyKeyError::InvalidCharacter),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn idempotency_key_conversions() -> Result<(), Box<dyn std::error::Error>> {
        let key: IdempotencyKey = "pay-1".parse()?;
        assert_eq!(key.to_string(), "pay-1");
        assert_eq!(IdempotencyKey::try_from("pay-1".to_owned())?, key);
        assert_eq!(String::from(key.clone()), "pay-1");
        assert_eq!(serde_json::to_string(&key)?, "\"pay-1\"");
        assert!(serde_json::from_str::<IdempotencyKey>("\"a b\"").is_err());
        assert_eq!(
            IdempotencyKeyError::Empty.to_string(),
            "idempotency key is empty"
        );
        assert_eq!(
            IdempotencyKeyError::TooLong.to_string(),
            "idempotency key is longer than 128 characters"
        );
        assert_eq!(
            IdempotencyKeyError::InvalidCharacter.to_string(),
            "idempotency key has an invalid character"
        );
        Ok(())
    }
}

#[cfg(test)]
mod properties {
    use proptest::prelude::*;

    use super::*;

    fn v7_uuid() -> impl Strategy<Value = Uuid> {
        any::<[u8; 16]>().prop_map(|mut bytes| {
            bytes[6] = (bytes[6] & 0x0f) | 0x70;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            Uuid::from_bytes(bytes)
        })
    }

    proptest! {
        #[test]
        fn v7_ids_round_trip_through_text(uuid in v7_uuid()) {
            let id = AccountId::from_uuid(uuid);
            prop_assert!(id.is_ok());
            let text = id.map(|id| id.to_string()).unwrap_or_default();
            prop_assert_eq!(text.parse::<AccountId>(), id);
        }

        #[test]
        fn non_v7_ids_are_rejected(bytes in any::<[u8; 16]>()) {
            let uuid = Uuid::from_bytes(bytes);
            let is_v7 = uuid.get_version_num() == 7 && uuid.get_variant() == Variant::RFC4122;
            prop_assert_eq!(EntryId::from_uuid(uuid).is_ok(), is_v7);
        }

        #[test]
        fn id_parsing_never_panics(s in ".{0,50}") {
            let _ = s.parse::<HoldId>();
        }

        #[test]
        fn idempotency_key_never_panics(s in ".{0,200}") {
            if let Ok(key) = IdempotencyKey::new(s.clone()) {
                prop_assert_eq!(key.as_str(), s.as_str());
            }
        }
    }
}

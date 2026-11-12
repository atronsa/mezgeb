//! Who corrected the books, and why (step 1.12, ADR-0011, INV-25).
//!
//! History is never edited: a mistake is corrected by a reversal (the exact
//! mirror of a wrong entry) or an adjustment (a new entry, such as E14's
//! goodwill credit). Both carry a reason code and the person who asked for
//! them. An adjustment can create money in a customer's wallet without any
//! payment behind it, so it also needs a second, different person to
//! approve it (maker-checker).

use std::fmt;
use std::str::FromStr;

use crate::model::LedgerError;

/// What kind of entry this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A normal business event: a deposit, payment, payout, refund.
    Standard,
    /// The exact mirror of an entry posted by mistake (INV-14).
    Reversal,
    /// A manual correction or compensation, approved by a second person
    /// (INV-25).
    Adjustment,
}

impl EntryKind {
    /// The name used in storage and the API.
    pub const fn as_str(self) -> &'static str {
        match self {
            EntryKind::Standard => "standard",
            EntryKind::Reversal => "reversal",
            EntryKind::Adjustment => "adjustment",
        }
    }

    /// The inverse of [`EntryKind::as_str`].
    pub fn from_name(name: &str) -> Option<EntryKind> {
        [
            EntryKind::Standard,
            EntryKind::Reversal,
            EntryKind::Adjustment,
        ]
        .into_iter()
        .find(|k| k.as_str() == name)
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors from validating an actor ID or a reason code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    InvalidCharacter,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NameError::Empty => "name is empty",
            NameError::TooLong => "name is too long",
            NameError::InvalidCharacter => "name has an invalid character",
        })
    }
}

impl std::error::Error for NameError {}

fn check(value: &str, max: usize, allowed: impl Fn(u8) -> bool) -> Result<(), NameError> {
    if value.is_empty() {
        Err(NameError::Empty)
    } else if value.len() > max {
        Err(NameError::TooLong)
    } else if !value.bytes().all(allowed) {
        Err(NameError::InvalidCharacter)
    } else {
        Ok(())
    }
}

/// The person or system that asked for or approved a correction, as the
/// client application identifies them (for example `"agent:42"`).
///
/// 1 to 128 characters from `A–Z a–z 0–9 - _ . : @`. The ledger records
/// it; proving who the actor really is belongs to authentication (step 2.08).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActorId(String);

impl ActorId {
    pub fn new(id: impl Into<String>) -> Result<ActorId, NameError> {
        let id = id.into();
        check(&id, 128, |b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'@')
        })?;
        Ok(ActorId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ActorId {
    type Err = NameError;

    fn from_str(s: &str) -> Result<ActorId, NameError> {
        ActorId::new(s)
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A reason code such as `"booking_error"` or `"goodwill_credit"`:
/// 1 to 64 characters from `a–z 0–9 _`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReasonCode(String);

impl ReasonCode {
    pub fn new(code: impl Into<String>) -> Result<ReasonCode, NameError> {
        let code = code.into();
        check(&code, 64, |b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'
        })?;
        Ok(ReasonCode(code))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The kind of entry this code may be used for, if it is one of the
    /// [`REASON_CODES`].
    pub fn kind(&self) -> Option<EntryKind> {
        REASON_CODES
            .iter()
            .find(|(code, ..)| *code == self.0)
            .map(|(_, kind, _)| *kind)
    }
}

impl FromStr for ReasonCode {
    type Err = NameError;

    fn from_str(s: &str) -> Result<ReasonCode, NameError> {
        ReasonCode::new(s)
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The reason codes, with the kind of entry each may be used for. The same
/// list is seeded into the `reason_codes` table (migration 0003); a test
/// keeps the two equal.
pub const REASON_CODES: [(&str, EntryKind, &str); 7] = [
    (
        "booking_error",
        EntryKind::Reversal,
        "Posted by mistake: wrong account, amount or customer",
    ),
    (
        "duplicate_posting",
        EntryKind::Reversal,
        "The same event was posted twice",
    ),
    (
        "goodwill_credit",
        EntryKind::Adjustment,
        "Goodwill credit to a customer, paid from the platform's own money",
    ),
    (
        "compensation",
        EntryKind::Adjustment,
        "Compensation owed to a customer or merchant",
    ),
    (
        "bank_charge",
        EntryKind::Adjustment,
        "A bank charge booked after the fact",
    ),
    (
        "write_off",
        EntryKind::Adjustment,
        "An amount that will not be recovered",
    ),
    (
        "data_migration",
        EntryKind::Adjustment,
        "A balance carried over from another system",
    ),
];

/// Who asked for a correction, who approved it, and why.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Authorization {
    pub reason: ReasonCode,
    /// The maker: the person who asked for the correction.
    pub requested_by: ActorId,
    /// The checker: the second person who approved it. Required for
    /// adjustments; optional for reversals.
    pub approved_by: Option<ActorId>,
}

impl Authorization {
    /// Checks the authorization for an entry of `kind`:
    ///
    /// - a standard entry has none;
    /// - a reversal has a reversal reason and a requester; an approver, if
    ///   given, must be someone else;
    /// - an adjustment has an adjustment reason, a requester and a different
    ///   approver (INV-25).
    pub fn check(
        kind: EntryKind,
        authorization: Option<&Authorization>,
    ) -> Result<(), LedgerError> {
        let Some(auth) = authorization else {
            return match kind {
                EntryKind::Standard => Ok(()),
                _ => Err(LedgerError::AuthorizationRequired),
            };
        };
        if kind == EntryKind::Standard {
            return Err(LedgerError::AuthorizationNotAllowed);
        }
        if auth.reason.kind() != Some(kind) {
            return Err(LedgerError::ReasonNotAllowed(
                auth.reason.as_str().to_owned(),
            ));
        }
        match &auth.approved_by {
            Some(checker) if *checker == auth.requested_by => Err(LedgerError::MakerIsChecker),
            None if kind == EntryKind::Adjustment => Err(LedgerError::AuthorizationRequired),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(reason: &str, maker: &str, checker: Option<&str>) -> Result<Authorization, NameError> {
        Ok(Authorization {
            reason: ReasonCode::new(reason)?,
            requested_by: ActorId::new(maker)?,
            approved_by: checker.map(ActorId::new).transpose()?,
        })
    }

    #[test]
    fn names_are_validated() {
        assert!(ActorId::new("agent:42").is_ok());
        assert!(ActorId::new("ops.lead@example").is_ok());
        assert_eq!(ActorId::new(""), Err(NameError::Empty));
        assert_eq!(ActorId::new("a".repeat(129)), Err(NameError::TooLong));
        assert_eq!(ActorId::new("a b"), Err(NameError::InvalidCharacter));
        assert!(ReasonCode::new("goodwill_credit").is_ok());
        assert_eq!(
            ReasonCode::new("Goodwill"),
            Err(NameError::InvalidCharacter)
        );
        assert_eq!(ReasonCode::new("x".repeat(65)), Err(NameError::TooLong));
        assert_eq!(
            "agent:1".parse::<ActorId>().map(|a| a.to_string()),
            Ok("agent:1".into())
        );
        assert_eq!(
            "write_off".parse::<ReasonCode>().map(|r| r.to_string()),
            Ok("write_off".into())
        );
        for (error, message) in [
            (NameError::Empty, "name is empty"),
            (NameError::TooLong, "name is too long"),
            (NameError::InvalidCharacter, "name has an invalid character"),
        ] {
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn reason_codes_are_unique_and_valid() {
        for (i, (code, ..)) in REASON_CODES.iter().enumerate() {
            assert!(ReasonCode::new(*code).is_ok(), "{code}");
            assert!(
                REASON_CODES[i + 1..]
                    .iter()
                    .all(|(other, ..)| other != code)
            );
        }
        assert_eq!(
            ReasonCode::new("goodwill_credit")
                .ok()
                .and_then(|r| r.kind()),
            Some(EntryKind::Adjustment)
        );
        assert_eq!(ReasonCode::new("made_up").ok().and_then(|r| r.kind()), None);
    }

    #[test]
    fn entry_kind_names_round_trip() {
        for kind in [
            EntryKind::Standard,
            EntryKind::Reversal,
            EntryKind::Adjustment,
        ] {
            assert_eq!(EntryKind::from_name(kind.as_str()), Some(kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert_eq!(EntryKind::from_name("other"), None);
    }

    #[test]
    fn authorization_rules() -> Result<(), NameError> {
        use EntryKind::*;
        let goodwill = auth("goodwill_credit", "agent:1", Some("lead:2"))?;
        let self_approved = auth("goodwill_credit", "agent:1", Some("agent:1"))?;
        let unapproved = auth("goodwill_credit", "agent:1", None)?;
        let reversal = auth("booking_error", "ops:1", None)?;
        let unknown = auth("made_up", "ops:1", None)?;

        let cases: [(EntryKind, Option<&Authorization>, Result<(), LedgerError>); 10] = [
            (Standard, None, Ok(())),
            (
                Standard,
                Some(&reversal),
                Err(LedgerError::AuthorizationNotAllowed),
            ),
            (Reversal, None, Err(LedgerError::AuthorizationRequired)),
            (Reversal, Some(&reversal), Ok(())),
            (
                Reversal,
                Some(&goodwill),
                Err(LedgerError::ReasonNotAllowed("goodwill_credit".into())),
            ),
            (
                Reversal,
                Some(&unknown),
                Err(LedgerError::ReasonNotAllowed("made_up".into())),
            ),
            (Adjustment, None, Err(LedgerError::AuthorizationRequired)),
            (Adjustment, Some(&goodwill), Ok(())),
            (
                Adjustment,
                Some(&unapproved),
                Err(LedgerError::AuthorizationRequired),
            ),
            (
                Adjustment,
                Some(&self_approved),
                Err(LedgerError::MakerIsChecker),
            ),
        ];
        for (kind, authorization, expected) in cases {
            assert_eq!(
                Authorization::check(kind, authorization),
                expected,
                "{kind}"
            );
        }
        // A reversal may also be approved, by someone else.
        let approved_reversal = auth("booking_error", "ops:1", Some("ops:2"))?;
        assert_eq!(
            Authorization::check(Reversal, Some(&approved_reversal)),
            Ok(())
        );
        let self_approved_reversal = auth("booking_error", "ops:1", Some("ops:1"))?;
        assert_eq!(
            Authorization::check(Reversal, Some(&self_approved_reversal)),
            Err(LedgerError::MakerIsChecker)
        );
        Ok(())
    }
}

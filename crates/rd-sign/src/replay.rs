//! Freshness: refusing a correctly signed document that is simply old.
//!
//! A signature says who wrote a document, never when. Without a freshness rule an attacker
//! who can answer an update or repository request replays yesterday's genuine, genuinely
//! signed manifest for as long as they like, and the installation never learns that a fix
//! exists. That is the whole attack, and it needs no key.
//!
//! Three rules, each covering a case the others do not:
//!
//! * **Sequence never goes backwards.** The publisher's counter is monotonic, so a document
//!   with a lower sequence than the one already seen is a replay by definition.
//! * **Expiry.** `not_after` bounds how long a replay stays useful even on a first contact,
//!   where there is no stored sequence to compare against.
//! * **A limit on future timestamps.** Clock skew is normal; a document issued a week from
//!   now is not, and accepting it would let a producer mint something that outlives its
//!   own expiry rule.

use chrono::{DateTime, Duration, Utc};

/// How far ahead of local time an `issued_at` may sit before it stops being clock skew.
pub const MAX_CLOCK_SKEW_MINUTES: i64 = 60;

/// The freshness fields every signed document in this project carries.
#[derive(Clone, Copy, Debug)]
pub struct Freshness {
    /// The publisher's monotonic counter for this document kind.
    pub sequence: u64,
    /// When the publisher says it signed this.
    pub issued_at: DateTime<Utc>,
    /// After this instant the document is stale even if nothing newer has been seen.
    pub not_after: Option<DateTime<Utc>>,
}

/// Why a document was refused as stale.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StaleError {
    /// A sequence at or below the highest one already accepted.
    #[error("document sequence {saw} is not newer than the {known} already accepted")]
    Replayed { saw: u64, known: u64 },
    /// Past its own expiry.
    #[error("document expired at {expired_at}")]
    Expired { expired_at: DateTime<Utc> },
    /// Issued further into the future than clock skew explains.
    #[error("document is issued {issued_at}, further ahead than clock skew explains")]
    FromTheFuture { issued_at: DateTime<Utc> },
}

/// Checks `candidate` against the highest sequence already accepted, if any.
///
/// `known` is `None` on a first contact, where only expiry and the skew bound apply.
pub fn check(
    candidate: Freshness,
    known: Option<u64>,
    now: DateTime<Utc>,
) -> Result<(), StaleError> {
    if let Some(known) = known
        && candidate.sequence <= known
    {
        return Err(StaleError::Replayed {
            saw: candidate.sequence,
            known,
        });
    }
    if let Some(expired_at) = candidate.not_after
        && now > expired_at
    {
        return Err(StaleError::Expired { expired_at });
    }
    if candidate.issued_at > now + Duration::minutes(MAX_CLOCK_SKEW_MINUTES) {
        return Err(StaleError::FromTheFuture {
            issued_at: candidate.issued_at,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_760_000_000 + minutes * 60, 0).expect("timestamp")
    }

    fn fresh(sequence: u64) -> Freshness {
        Freshness {
            sequence,
            issued_at: at(0),
            not_after: Some(at(60)),
        }
    }

    #[test]
    fn a_newer_sequence_is_accepted() {
        assert_eq!(check(fresh(8), Some(7), at(1)), Ok(()));
    }

    /// The replay case: yesterday's genuine manifest, served again.
    #[test]
    fn the_same_sequence_is_a_replay() {
        assert!(matches!(
            check(fresh(7), Some(7), at(1)),
            Err(StaleError::Replayed { .. })
        ));
    }

    #[test]
    fn an_older_sequence_is_a_replay() {
        assert!(matches!(
            check(fresh(6), Some(7), at(1)),
            Err(StaleError::Replayed { .. })
        ));
    }

    /// On a first contact there is nothing to compare against, so expiry is the only guard.
    #[test]
    fn a_first_contact_accepts_any_sequence_but_not_an_expired_document() {
        assert_eq!(check(fresh(1), None, at(1)), Ok(()));
        assert!(matches!(
            check(fresh(1), None, at(61)),
            Err(StaleError::Expired { .. })
        ));
    }

    /// Ordinary clock skew must not lock an installation out of its own updates.
    #[test]
    fn clock_skew_within_the_allowance_is_tolerated() {
        let candidate = Freshness {
            sequence: 1,
            issued_at: at(30),
            not_after: None,
        };
        assert_eq!(check(candidate, None, at(0)), Ok(()));
    }

    #[test]
    fn a_document_from_next_week_is_refused() {
        let candidate = Freshness {
            sequence: 1,
            issued_at: at(60 * 24 * 7),
            not_after: None,
        };
        assert!(matches!(
            check(candidate, None, at(0)),
            Err(StaleError::FromTheFuture { .. })
        ));
    }

    /// A document with no expiry is legitimate; only the sequence then protects it.
    #[test]
    fn a_document_without_an_expiry_is_governed_by_its_sequence_alone() {
        let candidate = Freshness {
            sequence: 9,
            issued_at: at(0),
            not_after: None,
        };
        assert_eq!(check(candidate, Some(8), at(60 * 24 * 365)), Ok(()));
    }
}

//! What pCloud said, reduced to something safe to repeat.
//!
//! pCloud answers **HTTP 200 to almost everything**, including to every refusal: what went
//! wrong is the `result` field of the JSON document, and `error` beside it is an English
//! sentence written for a developer. So the status code says almost nothing here, and the
//! number says everything.
//!
//! That turns RD-105-01's rule — never pass a provider's text through — from a filtering
//! problem into a non-problem: the only value that travels out of this module is a **decimal
//! integer**, which cannot carry a leaked token, a file name or a sentence. `error` is
//! deliberately never read at all; there is no shape check that makes a sentence safe.
//!
//! The mapping below is by pCloud's documented *family* rather than by individual numbers.
//! pCloud publishes the families and guarantees them — 4xxx is rate limiting, 5xxx is the
//! server, 7xxx is a public link nobody in this process is responsible for — while the
//! individual numbers inside a family are a longer and less stable list. Classifying by the
//! family is therefore the reading that stays true, and the number travels alongside so an
//! unfamiliar one is visible instead of being flattened into "no".

use serde_json::Value;

/// pCloud's success value.
pub const OK: u64 = 0;

/// `Log in required.`
pub const LOG_IN_REQUIRED: u64 = 1000;
/// `Log in failed.`
pub const LOG_IN_FAILED: u64 = 2000;
/// `Invalid 'access_token' provided.` — also what the *other* region answers to a token that
/// is perfectly valid in its own, which is what makes it the region retry's trigger.
pub const INVALID_ACCESS_TOKEN: u64 = 2094;
/// `Access denied. You do not have permissions to perform this operation.`
pub const ACCESS_DENIED: u64 = 2003;

/// What one pCloud `result` means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Category {
    /// `result: 0`.
    Ok,
    /// The token was refused. Either it is not a token, or it belongs to the other region.
    Credential,
    /// A file or folder that is not there — pCloud's 2xxx "invalid operation" family.
    Missing,
    /// This account may not do that.
    Denied,
    /// The caller asked wrongly — pCloud's 1xxx family, other than a missing log-in.
    BadRequest,
    /// 4xxx: pCloud is rate limiting this application or this address.
    RateLimited,
    /// 5xxx: pCloud's own servers.
    Unavailable,
    /// 7xxx: a public link that is gone, expired, out of traffic, or asking for a password.
    /// Also what the *other* region answers to a code that exists in its own.
    Link,
    /// A number outside every family this plugin has a case for.
    Unknown,
}

impl Category {
    /// The family `result` belongs to.
    #[must_use]
    pub const fn of(result: u64) -> Self {
        match result {
            OK => Self::Ok,
            LOG_IN_REQUIRED | LOG_IN_FAILED | INVALID_ACCESS_TOKEN => Self::Credential,
            ACCESS_DENIED => Self::Denied,
            1001..=1999 => Self::BadRequest,
            2001..=2999 => Self::Missing,
            4000..=4999 => Self::RateLimited,
            5000..=5999 => Self::Unavailable,
            7000..=7999 => Self::Link,
            _ => Self::Unknown,
        }
    }

    /// Whether this refusal is one the *other* region might not make.
    ///
    /// The whole of the region correction, and deliberately narrow: only a refused credential
    /// and a refused link code. Everything else — a missing file, a denied operation, a rate
    /// limit — means the region was right and the answer was no, and retrying it elsewhere
    /// would double every one of those requests for nothing.
    #[must_use]
    pub const fn may_be_the_other_region(self) -> bool {
        matches!(self, Self::Credential | Self::Link)
    }
}

/// The `result` of a pCloud answer, or `None` when the document is not one.
///
/// An HTTP status that is not 2xx never gets here: the caller turns that into its own refusal
/// first. What this reads is the ordinary answer, which is where pCloud puts the refusal.
#[must_use]
pub fn result_of(body: &[u8]) -> Option<u64> {
    let document: Value = serde_json::from_slice(body).ok()?;
    document.get("result").and_then(Value::as_u64)
}

/// How long pCloud asked to be left alone, when it said.
///
/// Read from the `Retry-After` response header, which is the only place a number appears; the
/// rate-limit document itself carries a sentence and no interval.
#[must_use]
pub fn retry_after(headers: &[(String, String)]) -> Option<u64> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::{Category, result_of, retry_after};

    #[test]
    fn a_result_is_read_out_of_the_document_pcloud_actually_sends() {
        assert_eq!(result_of(br#"{"result":0,"metadata":{}}"#), Some(0));
        assert_eq!(
            result_of(br#"{"result":2009,"error":"File not found."}"#),
            Some(2009)
        );
        assert_eq!(result_of(b"<html>502 Bad Gateway</html>"), None);
        assert_eq!(result_of(b"{}"), None);
    }

    /// The families a person acts on differently, told apart — and each of them a different
    /// category, or the interface could not tell somebody which of them happened.
    #[test]
    fn the_families_pcloud_refuses_with_are_told_apart() {
        assert_eq!(Category::of(0), Category::Ok);
        assert_eq!(Category::of(1000), Category::Credential);
        assert_eq!(Category::of(2000), Category::Credential);
        assert_eq!(Category::of(2094), Category::Credential);
        assert_eq!(Category::of(2003), Category::Denied);
        assert_eq!(Category::of(1004), Category::BadRequest);
        assert_eq!(Category::of(2009), Category::Missing);
        assert_eq!(Category::of(2005), Category::Missing);
        assert_eq!(Category::of(4000), Category::RateLimited);
        assert_eq!(Category::of(5001), Category::Unavailable);
        assert_eq!(Category::of(7003), Category::Link);
        assert_eq!(Category::of(9999), Category::Unknown);
    }

    /// Only a refused credential and a refused link code may be the other installation. A
    /// missing file is missing in both, and retrying it would double the request for nothing.
    #[test]
    fn only_the_two_refusals_that_can_mean_the_wrong_region_are_retried_elsewhere() {
        assert!(Category::of(2094).may_be_the_other_region());
        assert!(Category::of(7001).may_be_the_other_region());
        for settled in [0_u64, 2003, 2009, 4000, 5000, 1004, 9999] {
            assert!(
                !Category::of(settled).may_be_the_other_region(),
                "{settled}"
            );
        }
    }

    #[test]
    fn the_wait_is_read_from_the_header_because_the_document_states_none() {
        let headers = vec![("Retry-After".to_owned(), " 90 ".to_owned())];
        assert_eq!(retry_after(&headers), Some(90));
        assert_eq!(retry_after(&[]), None);
        assert_eq!(
            retry_after(&[("retry-after".to_owned(), "soon".to_owned())]),
            None
        );
    }
}

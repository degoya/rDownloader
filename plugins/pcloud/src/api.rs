//! What one pCloud refusal means.
//!
//! pCloud answers HTTP 200 to almost everything and puts the refusal in the `result` number,
//! so this is a mapping from that number's family — `pcloud_common::api::Category` — to the
//! stable code and the scheduler category a person acts on. Kept apart from the component so
//! `cargo test` covers it on the host target.

use pcloud_common::api::Category;

/// The failure categories this plugin reports, in the vocabulary both adapters share.
pub use plugin_common::FailureKind;

use crate::messages;

/// What one pCloud `result` means, as a stable code and a scheduler category.
///
/// `retry_after` is pCloud's own `Retry-After`, in seconds, when it sent one.
#[must_use]
pub fn classify(
    result: u64,
    retry_after: Option<u64>,
) -> ((&'static str, &'static str), FailureKind) {
    match Category::of(result) {
        // A refused credential, however pCloud spelled it — including a token that is perfectly
        // valid in the *other* installation. The caller has already tried the other one by the
        // time this is reported, so this really does mean "sign in again".
        Category::Credential => (messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired),
        // A rate limit is a property of the application and the account, not of one link:
        // reported as an IP block so the scheduler holds every pCloud link back for as long as
        // pCloud asked, instead of burning the wait on each of them in turn.
        Category::RateLimited => (messages::RATE_LIMITED, FailureKind::IpBlocked(retry_after)),
        Category::Missing => (messages::FILE_NOT_FOUND, FailureKind::Permanent),
        Category::Denied => (messages::DOWNLOAD_NOT_PERMITTED, FailureKind::Permanent),
        // The one family a person may be able to fix from outside: a link that is gone,
        // expired, out of traffic or asking for a password. pCloud's number says which.
        Category::Link => (messages::LINK_UNAVAILABLE, FailureKind::Permanent),
        Category::Unavailable => (messages::UNAVAILABLE, FailureKind::Transient(retry_after)),
        // A refusal pCloud numbered that this plugin has no case for, and the impossible one:
        // `result: 0` never reaches here, because a caller that got it has an answer.
        Category::BadRequest | Category::Unknown | Category::Ok => {
            (messages::API_REFUSED, FailureKind::Permanent)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FailureKind, classify};
    use crate::messages;

    /// The refusals a person acts on differently, told apart.
    #[test]
    fn the_ways_pcloud_says_no_are_told_apart() {
        let cases = [
            (2009_u64, messages::FILE_NOT_FOUND),
            (2003, messages::DOWNLOAD_NOT_PERMITTED),
            (2094, messages::SIGN_IN_REQUIRED),
            (7003, messages::LINK_UNAVAILABLE),
            (4000, messages::RATE_LIMITED),
            (5000, messages::UNAVAILABLE),
            (1004, messages::API_REFUSED),
        ];
        for (result, expected) in cases {
            assert_eq!(classify(result, None).0, expected, "{result}");
        }
        // And each of them is a *different* code, or the interface could not tell a person
        // which of them happened.
        let codes: std::collections::BTreeSet<&str> = cases
            .iter()
            .map(|(result, _)| classify(*result, None).0.0)
            .collect();
        assert_eq!(codes.len(), cases.len());
    }

    /// A rate limit holds the whole provider back for as long as pCloud asked, and nothing
    /// else; an outage is a wait for this one link.
    #[test]
    fn a_rate_limit_blocks_the_provider_and_carries_retry_after() {
        assert_eq!(
            classify(4000, Some(300)),
            (messages::RATE_LIMITED, FailureKind::IpBlocked(Some(300)))
        );
        assert_eq!(classify(5001, None).1, FailureKind::Transient(None));
    }

    #[test]
    fn a_refused_credential_and_an_unknown_refusal_end_the_call_differently() {
        assert_eq!(classify(1000, None).1, FailureKind::AuthRequired);
        assert_eq!(classify(9999, None).1, FailureKind::Permanent);
        assert_eq!(classify(9999, None).0, messages::API_REFUSED);
    }
}

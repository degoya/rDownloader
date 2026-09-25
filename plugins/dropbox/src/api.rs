//! What one Dropbox refusal means.
//!
//! Dropbox says "no" as a tagged union under HTTP 409, and the tags are what a person has to
//! act on differently: a link that wants a password, a file that is not there, a Paper document
//! that has no bytes. Telling them apart is the difference between a message somebody can do
//! something about and "Dropbox refused this request". Kept apart from the component so `cargo
//! test` covers it on the host target.

/// The failure categories this plugin reports, in the vocabulary both adapters share.
pub use plugin_common::FailureKind;

use crate::messages;

/// What one HTTP status plus one reason means, as a stable code and a scheduler category.
///
/// `retry_after` is the provider's own `Retry-After`, in seconds, when it sent one.
#[must_use]
pub fn classify(
    status: u16,
    reason: Option<&str>,
    retry_after: Option<u64>,
) -> ((&'static str, &'static str), FailureKind) {
    match (status, reason.unwrap_or_default()) {
        // A refused credential, whichever way Dropbox spelled it — including a token that was
        // issued before the app asked for a scope it now needs. `AuthRequired` for the
        // scheduler, which stops asking and tells the person to sign in again.
        (401, _) | (_, "invalid_access_token" | "expired_access_token" | "missing_scope") => {
            (messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
        }
        // A rate limit is a property of the app and the account, not of one link: reported as
        // an IP block so the scheduler holds every Dropbox link back for as long as Dropbox
        // asked, instead of burning the wait on each of them in turn — and holds back nothing
        // else.
        (429, _) | (_, "too_many_requests") => {
            (messages::RATE_LIMITED, FailureKind::IpBlocked(retry_after))
        }
        (404, _) | (_, "path/not_found" | "shared_link_not_found") => {
            (messages::FILE_NOT_FOUND, FailureKind::Permanent)
        }
        // The one refusal a person can fix from the address: a missing or wrong link password.
        (_, "shared_link_access_denied") => (messages::LINK_ACCESS_DENIED, FailureKind::Permanent),
        (_, "shared_link_is_directory" | "path/not_file") => {
            (messages::IS_A_FOLDER, FailureKind::Unsupported)
        }
        (_, "unsupported_link_type" | "path/malformed_path") => {
            (messages::NOT_A_DROPBOX_LINK, FailureKind::Unsupported)
        }
        // No bytes to serve, or none this account may have.
        (403, _)
        | (
            _,
            "path/restricted_content" | "unsupported_file" | "no_permission" | "insufficient_scope",
        ) => (messages::DOWNLOAD_NOT_PERMITTED, FailureKind::Permanent),
        (500..=599, _) => (messages::UNAVAILABLE, FailureKind::Transient(retry_after)),
        // A refusal Dropbox named that this plugin has no case for. Reported with its reason as
        // a parameter, so an unfamiliar one is visible instead of being flattened into "no".
        _ => (messages::API_REFUSED, FailureKind::Permanent),
    }
}

#[cfg(test)]
mod tests {
    use super::{FailureKind, classify};
    use crate::messages;

    /// The refusals a person acts on differently, told apart.
    #[test]
    fn the_ways_dropbox_says_no_are_told_apart() {
        let cases = [
            ("path/not_found", messages::FILE_NOT_FOUND),
            ("shared_link_access_denied", messages::LINK_ACCESS_DENIED),
            ("shared_link_is_directory", messages::IS_A_FOLDER),
            ("path/restricted_content", messages::DOWNLOAD_NOT_PERMITTED),
            ("unsupported_link_type", messages::NOT_A_DROPBOX_LINK),
            ("expired_access_token", messages::SIGN_IN_REQUIRED),
        ];
        for (dropbox_reason, expected) in cases {
            let (code, _) = classify(409, Some(dropbox_reason), None);
            assert_eq!(code, expected, "{dropbox_reason}");
        }
        // And each of them is a *different* code, or the interface could not tell a person
        // which of them happened.
        let codes: std::collections::BTreeSet<&str> = cases
            .iter()
            .map(|(dropbox_reason, _)| classify(409, Some(dropbox_reason), None).0.0)
            .collect();
        assert_eq!(codes.len(), cases.len());
    }

    /// A rate limit holds the whole provider back for as long as Dropbox asked, and nothing
    /// else; an outage is a wait for this one link.
    #[test]
    fn a_rate_limit_blocks_the_provider_and_carries_retry_after() {
        assert_eq!(
            classify(429, Some("too_many_requests"), Some(300)),
            (messages::RATE_LIMITED, FailureKind::IpBlocked(Some(300)))
        );
        assert_eq!(classify(503, None, None).1, FailureKind::Transient(None));
    }

    #[test]
    fn a_refused_credential_and_an_unknown_refusal_end_the_call_differently() {
        assert_eq!(classify(401, None, None).1, FailureKind::AuthRequired);
        assert_eq!(
            classify(409, Some("missing_scope"), None).1,
            FailureKind::AuthRequired
        );
        assert_eq!(
            classify(409, Some("something_new"), None),
            (messages::API_REFUSED, FailureKind::Permanent)
        );
    }
}

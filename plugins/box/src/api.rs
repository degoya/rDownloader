//! Reading what Box answered, and what one refusal means.
//!
//! Box says "no" with an HTTP status and a lowercase `code`, and the codes are what a person
//! has to act on differently: a link that wants a password, a file that is not there, an
//! account over its allowance. Telling them apart is the difference between a message somebody
//! can do something about and "Box refused this request". Kept apart from the component so
//! `cargo test` covers it on the host target.

use serde::Deserialize;

/// The failure categories this plugin reports, in the vocabulary both adapters share.
pub use plugin_common::FailureKind;

use crate::messages;

/// Box states sizes as numbers; being lenient costs nothing.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Flexible {
    Number(u64),
    Text(String),
}

impl Flexible {
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
        }
    }
}

/// The version of a file Box just described: `file_version` (mini).
#[derive(Debug, Default, Deserialize)]
pub struct FileVersion {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
}

/// One item as `/2.0/files/<id>`, `/2.0/folders/<id>` and `/2.0/shared_items` describe it.
#[derive(Debug, Default, Deserialize)]
pub struct Item {
    /// `file`, `folder` or `web_link`.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<Flexible>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub file_version: Option<FileVersion>,
    /// `active`, `trashed` or `deleted`.
    #[serde(default)]
    pub item_status: Option<String>,
}

impl Item {
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.kind.as_deref() == Some("file")
    }

    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.kind.as_deref() == Some("folder")
    }

    /// Whether Box has put this item in the trash or removed it. An item without a status is
    /// live: the field is only returned for the account's own items.
    #[must_use]
    pub fn is_gone(&self) -> bool {
        matches!(self.item_status.as_deref(), Some("trashed" | "deleted"))
    }

    /// The version of the bytes Box just described, when it stated one.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.file_version
            .as_ref()
            .and_then(|version| version.id.as_deref())
            .filter(|id| box_common::address::valid_version(id))
    }

    /// The SHA-1 Box states for this version, when it states one. Box computes it over the
    /// whole file, so it is a plain digest and is named as one.
    #[must_use]
    pub fn sha1(&self) -> Option<String> {
        self.sha1
            .as_deref()
            .or_else(|| {
                self.file_version
                    .as_ref()
                    .and_then(|version| version.sha1.as_deref())
            })
            .filter(|value| box_common::address::valid_sha1(value))
            .map(str::to_ascii_lowercase)
    }
}

/// The account, from `/2.0/users/me`.
#[derive(Debug, Default, Deserialize)]
pub struct User {
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Reads a single item, or `None` when the document is not one.
#[must_use]
pub fn item(body: &[u8]) -> Option<Item> {
    serde_json::from_slice(body).ok()
}

/// Reads the account, or `None` when the document is not one.
#[must_use]
pub fn user(body: &[u8]) -> Option<User> {
    serde_json::from_slice(body).ok()
}

/// What one HTTP status plus one Box code means, as a stable code and a scheduler category.
///
/// `retry_after` is Box's own `Retry-After`, in seconds, when it sent one. `shared` says the
/// address was reached through a shared link, which is the one thing the status alone cannot
/// tell: Box answers a wrong or missing shared-link password with the same `forbidden` it uses
/// for a file an account may not read, deliberately, so that a shared link cannot be probed for
/// whether its password is the only thing in the way. The plugin keeps that ambiguity and says
/// which link it was about instead of inventing a distinction Box does not make.
#[must_use]
pub fn classify(
    status: u16,
    code: Option<&str>,
    retry_after: Option<u64>,
    shared: bool,
) -> ((&'static str, &'static str), FailureKind) {
    match (status, code.unwrap_or_default()) {
        // A refused credential, whichever way Box spelled it. `AuthRequired` for the scheduler,
        // which stops asking and tells the person to sign in again. Ahead of the shared-link
        // case because a 401 is about the account's token rather than about the link: Box
        // refuses a link with 403 or 404.
        (401, _) | (_, "unauthorized" | "invalid_token" | "invalid_grant") => {
            (messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
        }
        // A rate limit is a property of the application and the account, not of one link:
        // reported as an IP block so the scheduler holds every Box link back for as long as Box
        // asked, instead of burning the wait on each of them in turn — and holds back nothing
        // else.
        (429, _) | (_, "rate_limit_exceeded") => {
            (messages::RATE_LIMITED, FailureKind::IpBlocked(retry_after))
        }
        // The one refusal a person can fix from the address, as far as Box will say so.
        (403 | 404, _) if shared => (messages::LINK_ACCESS_DENIED, FailureKind::Permanent),
        (404, _) | (_, "not_found" | "trashed") => {
            (messages::FILE_NOT_FOUND, FailureKind::Permanent)
        }
        (_, "storage_limit_exceeded" | "account_storage_limit_exceeded") => {
            (messages::QUOTA_EXCEEDED, FailureKind::Permanent)
        }
        (403, _)
        | (
            _,
            "forbidden" | "access_denied_insufficient_permissions" | "access_denied_item_locked",
        ) => (messages::DOWNLOAD_NOT_PERMITTED, FailureKind::Permanent),
        (500..=599, _) | (_, "internal_server_error" | "unavailable") => {
            (messages::UNAVAILABLE, FailureKind::Transient(retry_after))
        }
        // A refusal Box named that this plugin has no case for. Reported with its code as a
        // parameter, so an unfamiliar one is visible instead of being flattened into "no".
        _ => (messages::API_REFUSED, FailureKind::Permanent),
    }
}

#[cfg(test)]
mod tests {
    use super::{FailureKind, classify, item, user};
    use crate::messages;

    const FILE: &[u8] = br#"{"type":"file","id":"12345","name":"release.bin","size":1048576,
        "sha1":"AABBCCDDEEFF00112233445566778899AABBCCDD","etag":"3",
        "file_version":{"type":"file_version","id":"98765",
            "sha1":"aabbccddeeff00112233445566778899aabbccdd"},
        "item_status":"active","content_modified_at":"2026-09-01T10:00:00-07:00"}"#;

    #[test]
    fn a_file_document_yields_what_a_download_needs() {
        let file = item(FILE).expect("an item");
        assert!(file.is_file() && !file.is_folder() && !file.is_gone());
        assert_eq!(file.name.as_deref(), Some("release.bin"));
        assert_eq!(
            file.size.as_ref().and_then(super::Flexible::as_u64),
            Some(1_048_576)
        );
        assert_eq!(file.version(), Some("98765"));
        // Box states the SHA-1 in upper case on the item and lower case on the version; one
        // spelling reaches the verifier.
        assert_eq!(
            file.sha1().as_deref(),
            Some("aabbccddeeff00112233445566778899aabbccdd")
        );
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_file() {
        assert!(item(b"<html>502 Bad Gateway</html>").is_none());
        assert!(item(b"").is_none());
        assert!(user(b"").is_none());
        // A value that is not a version or a digest is dropped rather than passed on.
        let odd = item(br#"{"type":"file","sha1":"nope","file_version":{"id":"../1"}}"#)
            .expect("an item");
        assert_eq!(odd.version(), None);
        assert_eq!(odd.sha1(), None);
    }

    #[test]
    fn a_trashed_item_says_so() {
        let trashed =
            item(br#"{"type":"file","id":"1","item_status":"trashed"}"#).expect("an item");
        assert!(trashed.is_gone());
    }

    /// The refusals a person acts on differently, told apart.
    #[test]
    fn the_ways_box_says_no_are_told_apart() {
        let cases = [
            (404, "not_found", messages::FILE_NOT_FOUND),
            (403, "forbidden", messages::DOWNLOAD_NOT_PERMITTED),
            (401, "unauthorized", messages::SIGN_IN_REQUIRED),
            (429, "rate_limit_exceeded", messages::RATE_LIMITED),
            (403, "storage_limit_exceeded", messages::QUOTA_EXCEEDED),
            (500, "internal_server_error", messages::UNAVAILABLE),
        ];
        for (status, code, expected) in cases {
            let (found, _) = classify(status, Some(code), None, false);
            assert_eq!(found, expected, "{code}");
        }
        // And each of them is a *different* code, or the interface could not tell a person
        // which of them happened.
        let codes: std::collections::BTreeSet<&str> = cases
            .iter()
            .map(|(status, code, _)| classify(*status, Some(code), None, false).0.0)
            .collect();
        assert_eq!(codes.len(), cases.len());
    }

    /// Box answers a wrong shared-link password with the same refusal it uses for a file
    /// somebody may not read. Through a shared link that is said as a shared-link refusal,
    /// which is the one a person can act on.
    #[test]
    fn a_refusal_through_a_shared_link_is_reported_as_one() {
        for status in [403, 404] {
            assert_eq!(
                classify(status, Some("forbidden"), None, true).0,
                messages::LINK_ACCESS_DENIED,
                "{status}"
            );
            assert_ne!(
                classify(status, Some("forbidden"), None, false).0,
                messages::LINK_ACCESS_DENIED,
                "{status}"
            );
        }
        // A rate limit is still a rate limit, shared link or not.
        assert_eq!(
            classify(429, Some("rate_limit_exceeded"), Some(30), true).0,
            messages::RATE_LIMITED
        );
    }

    #[test]
    fn a_rate_limit_blocks_the_provider_and_carries_retry_after() {
        assert_eq!(
            classify(429, Some("rate_limit_exceeded"), Some(300), false),
            (messages::RATE_LIMITED, FailureKind::IpBlocked(Some(300)))
        );
        assert_eq!(
            classify(503, None, None, false).1,
            FailureKind::Transient(None)
        );
    }

    #[test]
    fn an_unknown_refusal_ends_the_call_with_its_own_code() {
        assert_eq!(
            classify(400, Some("something_new"), None, false),
            (messages::API_REFUSED, FailureKind::Permanent)
        );
    }
}

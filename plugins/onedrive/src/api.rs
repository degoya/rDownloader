//! Reading what Microsoft Graph answered.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it without
//! a WebAssembly toolchain, which is the only place the API's shapes — a `driveItem` that is a
//! file, a folder or a OneNote package by which facet it carries, a hash block that names
//! three algorithms of which one is nobody else's, a 200 that is not JSON at all — can be
//! pinned down.
//!
//! The important half of this file is [`classify`]. Graph says "no" with an error code that
//! a person has to act on differently each time — a link shared with somebody else, an item
//! that is gone, a tenant that refuses the account, a throttle that resets — and the code is
//! the only thing that tells them apart, because two of them arrive as the same HTTP 403.
//!
//! **What is deliberately not read.** A `driveItem` carries an `eTag` and a `cTag`, and the
//! job asks for them to be used correctly. The correct use in a download manager is as a
//! validator on the transfer, and the contract has no place for one: `resolved-download`
//! carries a URL, a name, a size and a checksum, nothing a resume could compare a tag to. So
//! the transfer validates the way it validates every other download — against the HTTP
//! `ETag` and `If-Range` of the `/content` answer, which is the same content version — and
//! this plugin does not pretend to a use it cannot carry through.

use serde::Deserialize;

/// The failure categories this plugin reports, in the vocabulary both adapters share.
pub use plugin_common::FailureKind;

use crate::messages;

/// Graph states sizes as numbers; being lenient costs nothing and a quoted one would
/// otherwise read as "no size".
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

/// The digests Graph states for a file, in the two algorithms the checksum verifier knows.
/// `sha1Hash` is stated for personal accounts, `sha256Hash` where the tenant computes one; the
/// `quickXorHash` beside them is Microsoft's own construction and is not read.
#[derive(Debug, Default, Deserialize)]
pub struct Hashes {
    #[serde(rename = "sha1Hash", default)]
    pub sha1: Option<String>,
    #[serde(rename = "sha256Hash", default)]
    pub sha256: Option<String>,
}

/// The `file` facet: present exactly when the item is a file.
#[derive(Debug, Default, Deserialize)]
pub struct FileFacet {
    #[serde(default)]
    pub hashes: Option<Hashes>,
}

/// `GET /shares/{id}/driveItem`, `GET /shares/{id}/items/{item}`, `GET /drives/{d}/items/{i}`.
///
/// Which facet is present is what the item *is*: `folder` a folder, `file` a file, neither a
/// OneNote notebook — a `package`, which carries no bytes and is not read further. A parser
/// that read `file` as "has bytes" without asking would call a notebook a file of unknown
/// length.
#[derive(Debug, Deserialize)]
pub struct DriveItem {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<Flexible>,
    #[serde(default)]
    pub file: Option<FileFacet>,
    /// The `folder` facet, present exactly when the item is a folder. Its `childCount` is not
    /// read: the crawler sibling lists the children, and a count would only be a promise.
    #[serde(default)]
    pub folder: Option<serde_json::Value>,
    #[serde(default)]
    pub deleted: Option<serde_json::Value>,
}

impl DriveItem {
    /// The checksum Graph stated in an algorithm the verifier knows, strongest first.
    ///
    /// `quickXorHash` is not one of them: it is Microsoft's own construction, the checksum
    /// verifier has no implementation of it, and handing it on as `sha1` would fail every
    /// download whose bytes were perfectly fine.
    #[must_use]
    pub fn checksum(&self) -> Option<(String, String)> {
        let hashes = self.file.as_ref()?.hashes.as_ref()?;
        if let Some(value) = hashes.sha256.as_ref().filter(|v| is_hex(v, 64)) {
            return Some(("sha256".to_owned(), value.to_ascii_lowercase()));
        }
        hashes
            .sha1
            .as_ref()
            .filter(|value| is_hex(value, 40))
            .map(|value| ("sha1".to_owned(), value.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.folder.is_some()
    }

    #[must_use]
    pub fn is_file(&self) -> bool {
        self.file.is_some() && self.folder.is_none()
    }

    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.deleted.is_some()
    }
}

/// A hexadecimal digest of exactly the algorithm's length — the value is passed on to the
/// checksum verifier, so a truncated or mislabelled one has to be dropped here.
fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `GET /me/drive?$select=id,driveType,owner`.
///
/// Only the account's own identity is asked for. The drive's `quota` is deliberately not
/// read: it is space left to *upload* into, and an account row that showed it where remaining
/// download traffic goes would tell somebody with a full OneDrive that they cannot download
/// from it — which is not true, and is exactly the kind of wrong number a row is believed on
/// sight.
#[derive(Debug, Deserialize)]
pub struct Drive {
    #[serde(default)]
    pub owner: Option<IdentitySet>,
}

#[derive(Debug, Default, Deserialize)]
pub struct IdentitySet {
    #[serde(default)]
    pub user: Option<Identity>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Identity {
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// Reads an item, or `None` when the document is not one.
#[must_use]
pub fn item(body: &[u8]) -> Option<DriveItem> {
    serde_json::from_slice(body).ok()
}

/// Reads a drive, or `None`.
#[must_use]
pub fn drive(body: &[u8]) -> Option<Drive> {
    serde_json::from_slice(body).ok()
}

/// What one HTTP status plus one error code means, as a stable code and a scheduler category.
///
/// `retry_after` is the provider's own `Retry-After`, in seconds, when it sent one — and Graph
/// sends one with every throttle, which is the only number this plugin will wait on.
#[must_use]
pub fn classify(
    status: u16,
    code: Option<&str>,
    retry_after: Option<u64>,
) -> ((&'static str, &'static str), FailureKind) {
    match (status, code.unwrap_or_default()) {
        // A refused credential, whichever way it was spelled: the API's own code, or the
        // sign-in layer's. `AuthRequired` for the scheduler, which stops asking and tells the
        // person to sign in again.
        (401, _) | (_, "unauthenticated" | "InvalidAuthenticationToken" | "CompactToken") => {
            (messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
        }
        (404, _) | (_, "itemNotFound") => (messages::ITEM_NOT_FOUND, FailureKind::Permanent),
        // The tenant or the permission said no. Told apart from "not there" because a person
        // acts differently: a link made for another account needs that account, or the tenant
        // has to allow the sign-in — nothing about waiting or retrying helps.
        (_, "accessDenied") | (403, "") => (messages::ACCESS_DENIED, FailureKind::Permanent),
        // Viewing allowed, downloading not: a SharePoint policy, never a transient state.
        (_, "notAllowed") => (messages::DOWNLOAD_NOT_PERMITTED, FailureKind::Permanent),
        (_, "malwareDetected") => (messages::MALWARE_DETECTED, FailureKind::Permanent),
        // Graph could not decode the sharing link, or it names a tenant this account cannot
        // reach. The address is wrong for this account and stays wrong.
        (_, "invalidRequest") | (400, _) => (messages::INVALID_REQUEST, FailureKind::Permanent),
        (429, _) | (_, "activityLimitReached" | "tooManyRequests") => (
            messages::RATE_LIMITED,
            FailureKind::RateLimited(retry_after),
        ),
        (500..=599, _) | (_, "serviceNotAvailable") => {
            (messages::UNAVAILABLE, FailureKind::Transient(retry_after))
        }
        // A refusal Graph named that this plugin has no case for. Reported with its code as a
        // parameter, so an unfamiliar one is visible instead of being flattened into "no".
        _ => (messages::API_REFUSED, FailureKind::Permanent),
    }
}

#[cfg(test)]
mod tests {
    use super::{FailureKind, classify, drive, item};
    use crate::messages;

    const FILE: &[u8] = br#"{
      "id": "01BYE5RZ6QN3ZWBTUFOFD3GSPGOHDJD36K", "name": "release.bin", "size": 1048576,
      "eTag": "\"{6FBF2E7F-2B3E-4A53-A9CB-1A2B3C4D5E6F},2\"", "cTag": "\"c:{6FBF2E7F},1\"",
      "file": {"mimeType": "application/octet-stream",
               "hashes": {"quickXorHash": "MjJhOTk4ZjM0NWQ2NzA4OTAwMDAwMDAwMDAwMDAwMDA=",
                          "sha1Hash": "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709"}},
      "parentReference": {"driveId": "b!abc", "id": "01BYE5RZ56Y2GOVW7725BZO354PWSELRRZ"},
      "@microsoft.graph.downloadUrl": "https://example.invalid/short-lived?tempauth=x"
    }"#;
    // The fixture keeps the fields Graph actually sends, including the ones this plugin does
    // not read: a parser that broke on an unexpected field would break on Graph's next one.

    #[test]
    fn a_file_is_read_with_its_size_and_the_checksum_the_verifier_knows_lowercased() {
        let read = item(FILE).expect("item");
        assert!(read.is_file() && !read.is_folder() && !read.is_deleted());
        assert_eq!(read.name.as_deref(), Some("release.bin"));
        assert_eq!(
            read.size.as_ref().and_then(super::Flexible::as_u64),
            Some(1_048_576)
        );
        // SHA-1, lowercased. The QuickXorHash beside it is Microsoft's own and is not handed on
        // as anything.
        assert_eq!(
            read.checksum(),
            Some((
                "sha1".to_owned(),
                "da39a3ee5e6b4b0d3255bfef95601890afd80709".to_owned()
            ))
        );
    }

    /// SHA-256 wins over SHA-1 when a tenant states both, and a digest of the wrong length is
    /// not the digest it is labelled as.
    #[test]
    fn the_strongest_stated_digest_is_chosen_and_a_wrong_length_is_dropped() {
        let both = item(
            br#"{"name":"a","file":{"hashes":{"sha1Hash":"da39a3ee5e6b4b0d3255bfef95601890afd80709",
                 "sha256Hash":"E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"}}}"#,
        )
        .expect("item");
        assert_eq!(
            both.checksum().map(|(algorithm, _)| algorithm).as_deref(),
            Some("sha256")
        );
        let short =
            item(br#"{"name":"a","file":{"hashes":{"sha1Hash":"da39a3ee"}}}"#).expect("item");
        assert_eq!(short.checksum(), None);
        let only_xor =
            item(br#"{"name":"a","file":{"hashes":{"quickXorHash":"MjJhOTk4ZjM0NWQ2"}}}"#)
                .expect("item");
        assert_eq!(only_xor.checksum(), None);
    }

    /// The facet says what an item is. A folder has no bytes, a package is a OneNote notebook,
    /// and a deleted item is a tombstone whatever else it carries.
    #[test]
    fn the_facet_says_what_an_item_is() {
        let folder =
            item(br#"{"id":"x","name":"Season 1","folder":{"childCount":12}}"#).expect("item");
        assert!(folder.is_folder() && !folder.is_file());
        let notebook =
            item(br#"{"id":"x","name":"Notes","package":{"type":"oneNote"}}"#).expect("item");
        assert!(!notebook.is_folder() && !notebook.is_file());
        let gone = item(br#"{"id":"x","name":"a.bin","file":{},"deleted":{"state":"deleted"}}"#)
            .expect("item");
        assert!(gone.is_deleted());
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_item() {
        assert!(item(b"<html>502 Bad Gateway</html>").is_none());
        assert!(drive(b"").is_none());
    }

    #[test]
    fn a_drive_answer_carries_the_account_and_nothing_that_reads_as_traffic() {
        let answer = drive(
            br#"{"id":"b!abc","driveType":"business",
                 "owner":{"user":{"displayName":"Someone","email":"someone@example.invalid"}},
                 "quota":{"total":1099511627776,"used":1073741824,"remaining":1098437885952}}"#,
        )
        .expect("drive");
        assert_eq!(
            answer
                .owner
                .and_then(|owner| owner.user)
                .and_then(|user| user.email)
                .as_deref(),
            Some("someone@example.invalid")
        );
    }

    /// The ways Graph says no that a person acts on differently, told apart — including the
    /// two that both arrive as 403.
    #[test]
    fn the_ways_graph_says_no_are_told_apart() {
        let cases = [
            (403, "accessDenied", messages::ACCESS_DENIED),
            (403, "notAllowed", messages::DOWNLOAD_NOT_PERMITTED),
            (403, "malwareDetected", messages::MALWARE_DETECTED),
            (404, "itemNotFound", messages::ITEM_NOT_FOUND),
            (400, "invalidRequest", messages::INVALID_REQUEST),
            (
                401,
                "InvalidAuthenticationToken",
                messages::SIGN_IN_REQUIRED,
            ),
        ];
        for (status, graph_code, expected) in cases {
            let (code, _) = classify(status, Some(graph_code), None);
            assert_eq!(code, expected, "{graph_code}");
        }
        // And each of them is a *different* code, or the interface could not tell a person
        // which of them happened.
        let codes: std::collections::BTreeSet<&str> = cases
            .iter()
            .map(|(status, graph_code, _)| classify(*status, Some(graph_code), None).0.0)
            .collect();
        assert_eq!(codes.len(), cases.len());
    }

    /// A throttle is a wait carrying Microsoft's own `Retry-After`; an outage is a retry
    /// without a promise.
    #[test]
    fn a_throttle_waits_as_long_as_graph_asked() {
        assert_eq!(
            classify(429, Some("activityLimitReached"), Some(90)),
            (messages::RATE_LIMITED, FailureKind::RateLimited(Some(90)))
        );
        assert_eq!(
            classify(503, Some("serviceNotAvailable"), None),
            (messages::UNAVAILABLE, FailureKind::Transient(None))
        );
    }

    #[test]
    fn a_refused_credential_and_a_missing_item_end_the_call_differently() {
        assert_eq!(classify(401, None, None).1, FailureKind::AuthRequired);
        assert_eq!(classify(404, None, None).0, messages::ITEM_NOT_FOUND);
        // A 403 with a code this plugin has no case for is reported by that code, not as
        // "access denied" — the person has to be able to see what actually happened.
        assert_eq!(
            classify(403, Some("resyncRequired"), None).0,
            messages::API_REFUSED
        );
    }
}

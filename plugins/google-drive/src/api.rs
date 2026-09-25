//! Reading what the Drive API answered.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it without
//! a WebAssembly toolchain, which is the only place the API's odd shapes — a size quoted as a
//! string, an error document with no reason in it, a 200 that is not JSON at all — can be
//! pinned down.
//!
//! The important half of this file is [`classify`]. Drive says "no" in a dozen ways that a
//! person has to act on differently — a quota that resets tomorrow, an owner who switched
//! downloading off, a file Google could not scan — and every one of them arrives as HTTP 403.
//! Telling them apart is the difference between a message somebody can do something about and
//! "Google Drive refused this request".

use serde::Deserialize;

/// The failure categories this plugin reports, in the vocabulary both adapters share.
pub use plugin_common::FailureKind;

use crate::messages;

/// Drive quotes byte counts as JSON strings, and a `Number` in a few older fields.
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

/// What the account may do with one file. Only the one bit that decides a download is read.
#[derive(Debug, Default, Deserialize)]
pub struct Capabilities {
    #[serde(rename = "canDownload", default)]
    pub can_download: Option<bool>,
}

/// `GET /drive/v3/files/<id>?fields=…`.
#[derive(Debug, Deserialize)]
pub struct FileMetadata {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size: Option<Flexible>,
    #[serde(rename = "md5Checksum", default)]
    pub md5_checksum: Option<String>,
    #[serde(rename = "sha256Checksum", default)]
    pub sha256_checksum: Option<String>,
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
    #[serde(default)]
    pub trashed: Option<bool>,
}

impl FileMetadata {
    /// The checksum Drive stated, strongest first, or `None`.
    ///
    /// Drive reports these for uploaded binaries only; a Workspace export has none, because the
    /// bytes do not exist until the export runs.
    #[must_use]
    pub fn checksum(&self) -> Option<(String, String)> {
        if let Some(value) = self.sha256_checksum.as_ref().filter(|v| is_hex(v)) {
            return Some(("sha256".to_owned(), value.to_ascii_lowercase()));
        }
        self.md5_checksum
            .as_ref()
            .filter(|value| is_hex(value))
            .map(|value| ("md5".to_owned(), value.to_ascii_lowercase()))
    }

    /// Whether the account may download these bytes at all.
    ///
    /// Absent means yes: `capabilities` is only present when it was asked for, and treating a
    /// missing field as a refusal would fail every call that did not request it.
    #[must_use]
    pub fn can_download(&self) -> bool {
        self.capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.can_download)
            .unwrap_or(true)
    }
}

/// A hexadecimal digest and nothing else — the value is passed on to the checksum verifier.
fn is_hex(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `GET /drive/v3/about?fields=user,storageQuota`.
/// `GET /drive/v3/about?fields=user(...)`.
///
/// Only the account's own identity is asked for. Drive's `storageQuota` is deliberately not
/// read: it is space left to *upload* into, and an account row that showed it where remaining
/// download traffic goes would tell somebody with a full Drive that they cannot download from
/// it — which is not true, and is exactly the kind of wrong number a row is believed on sight.
#[derive(Debug, Deserialize)]
pub struct About {
    #[serde(default)]
    pub user: Option<AboutUser>,
}

#[derive(Debug, Deserialize)]
pub struct AboutUser {
    #[serde(rename = "emailAddress", default)]
    pub email_address: Option<String>,
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
}

/// Reads file metadata, or `None` when the document is not that.
#[must_use]
pub fn file(body: &[u8]) -> Option<FileMetadata> {
    serde_json::from_slice(body).ok()
}

/// Reads an `about` answer, or `None`.
#[must_use]
pub fn about(body: &[u8]) -> Option<About> {
    serde_json::from_slice(body).ok()
}

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
        // A refused credential, whichever way Google spelled it. `Failed` for the scheduler,
        // which stops asking and tells the person to sign in again.
        (401, _) | (_, "authError" | "unauthorized") => {
            (messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
        }
        (404, _) | (_, "notFound" | "fileNotFound") => {
            (messages::FILE_NOT_FOUND, FailureKind::Permanent)
        }
        // The three 403s that are not about permission at all. Told apart because a person acts
        // differently on each: a quota resets, a scan warning needs a decision, and an owner's
        // download switch never changes by waiting.
        (_, "downloadQuotaExceeded" | "quotaExceeded" | "storageQuotaExceeded") => (
            messages::QUOTA_EXCEEDED,
            // Not `RateLimited`: Drive's per-file quota resets on Google's own schedule, not
            // after a number of seconds anybody is told. Retried later, never sooner.
            FailureKind::Transient(retry_after),
        ),
        (_, "cannotDownloadAbusiveFile") => (messages::VIRUS_SCAN_WARNING, FailureKind::Permanent),
        (_, "cannotDownloadFile" | "insufficientFilePermissions" | "forbidden") => {
            (messages::DOWNLOAD_NOT_PERMITTED, FailureKind::Permanent)
        }
        (_, "exportSizeLimitExceeded") => (messages::EXPORT_TOO_LARGE, FailureKind::Permanent),
        (429, _) | (_, "rateLimitExceeded" | "userRateLimitExceeded" | "dailyLimitExceeded") => (
            messages::RATE_LIMITED,
            FailureKind::RateLimited(retry_after),
        ),
        (500..=599, _) => (messages::UNAVAILABLE, FailureKind::Transient(retry_after)),
        // A refusal Google named that this plugin has no case for. Reported with its reason as
        // a parameter, so an unfamiliar one is visible instead of being flattened into "no".
        _ => (messages::API_REFUSED, FailureKind::Permanent),
    }
}

#[cfg(test)]
mod tests {
    use super::{FailureKind, about, classify, file};
    use crate::messages;

    const METADATA: &[u8] = br#"{
      "id": "1A2b3C", "name": "release.bin", "mimeType": "application/octet-stream",
      "size": "1048576", "md5Checksum": "D41D8CD98F00B204E9800998ECF8427E",
      "capabilities": {"canDownload": true}, "modifiedTime": "2026-01-02T03:04:05.000Z"
    }"#;
    // The fixture keeps the fields Drive actually sends, including the two this plugin does
    // not read: a parser that broke on an unexpected field would break on Drive's next one.

    #[test]
    fn file_metadata_is_read_with_its_size_as_a_number_and_its_checksum_lowercased() {
        let metadata = file(METADATA).expect("metadata");
        assert_eq!(metadata.name.as_deref(), Some("release.bin"));
        assert_eq!(
            metadata.size.as_ref().and_then(super::Flexible::as_u64),
            Some(1_048_576)
        );
        assert_eq!(
            metadata.checksum(),
            Some((
                "md5".to_owned(),
                "d41d8cd98f00b204e9800998ecf8427e".to_owned()
            ))
        );
        assert!(metadata.can_download());
    }

    /// A digest that is not one never reaches the checksum verifier.
    #[test]
    fn a_checksum_that_is_not_hexadecimal_is_dropped() {
        let metadata = file(br#"{"name":"a","md5Checksum":"not a digest"}"#).expect("metadata");
        assert_eq!(metadata.checksum(), None);
        // And absent capabilities mean "not asked for", never "refused".
        assert!(metadata.can_download());
    }

    #[test]
    fn a_file_the_account_may_not_download_says_so_in_its_capabilities() {
        let metadata =
            file(br#"{"name":"a","capabilities":{"canDownload":false}}"#).expect("metadata");
        assert!(!metadata.can_download());
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_file() {
        assert!(file(b"<html>502 Bad Gateway</html>").is_none());
        assert!(about(b"").is_none());
    }

    #[test]
    fn an_about_answer_carries_the_account_and_nothing_that_reads_as_traffic() {
        let answer = about(
            br#"{"user":{"emailAddress":"someone@example.invalid","displayName":"Someone"},
                 "storageQuota":{"limit":"16106127360","usage":"1073741824"}}"#,
        )
        .expect("about");
        assert_eq!(
            answer.user.and_then(|user| user.email_address).as_deref(),
            Some("someone@example.invalid")
        );
    }

    /// The three 403s a person acts on differently, told apart.
    #[test]
    fn the_ways_drive_says_no_are_told_apart() {
        let cases = [
            ("downloadQuotaExceeded", messages::QUOTA_EXCEEDED),
            ("cannotDownloadAbusiveFile", messages::VIRUS_SCAN_WARNING),
            ("cannotDownloadFile", messages::DOWNLOAD_NOT_PERMITTED),
            ("exportSizeLimitExceeded", messages::EXPORT_TOO_LARGE),
        ];
        for (drive_reason, expected) in cases {
            let (code, _) = classify(403, Some(drive_reason), None);
            assert_eq!(code, expected, "{drive_reason}");
        }
        // And each of them is a *different* code, or the interface could not tell a person
        // which of them happened.
        let codes: std::collections::BTreeSet<&str> = cases
            .iter()
            .map(|(drive_reason, _)| classify(403, Some(drive_reason), None).0.0)
            .collect();
        assert_eq!(codes.len(), cases.len());
    }

    /// A rate limit is a wait carrying Google's own `Retry-After`; a quota is not the same
    /// thing and must not be retried in ninety seconds.
    #[test]
    fn a_rate_limit_waits_and_a_quota_does_not_pretend_to_know_how_long() {
        assert_eq!(
            classify(429, Some("userRateLimitExceeded"), Some(90)),
            (messages::RATE_LIMITED, FailureKind::RateLimited(Some(90)))
        );
        assert_eq!(
            classify(403, Some("downloadQuotaExceeded"), None).1,
            FailureKind::Transient(None)
        );
    }

    #[test]
    fn a_refused_credential_and_a_missing_file_end_the_call_differently() {
        assert_eq!(classify(401, None, None).1, FailureKind::AuthRequired);
        assert_eq!(
            classify(404, Some("notFound"), None).0,
            messages::FILE_NOT_FOUND
        );
        assert_eq!(classify(503, None, None).1, FailureKind::Transient(None));
    }
}

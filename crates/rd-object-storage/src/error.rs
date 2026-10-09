//! Mapping of object storage failures onto the queue's stable error codes.
//!
//! The service's own error text is never carried into a failure: it names the request URL,
//! and with it the bucket, the key and — for a presigning service — more than that. The code
//! and a bucket parameter are what the client translates.

use rd_core::{Failure, FailureKind, ProfileChoiceError};

/// No enabled profile can serve the link's bucket.
pub const NO_PROFILE: &str = "object_storage.no_profile";
/// More than one profile could serve it, and none is bound to the bucket.
pub const PROFILE_AMBIGUOUS: &str = "object_storage.profile_ambiguous";
/// The link names a profile that is switched off.
pub const PROFILE_DISABLED: &str = "object_storage.profile_disabled";
/// The link is not a valid object storage address.
pub const ADDRESS_INVALID: &str = "object_storage.address_invalid";
/// The object or the bucket does not exist.
pub const NOT_FOUND: &str = "object_storage.not_found";
/// The credentials are valid but not allowed to do this.
pub const ACCESS_DENIED: &str = "object_storage.access_denied";
/// The credentials were refused: unknown key, wrong secret, expired token.
pub const AUTH_FAILED: &str = "object_storage.auth_failed";
/// The endpoint could not be reached, or its certificate did not validate.
pub const CONNECT_FAILED: &str = "object_storage.connect_failed";
/// The object changed since the partial download was written.
pub const OBJECT_CHANGED: &str = "object_storage.object_changed";
/// A key in a listing would escape the download folder.
pub const UNSAFE_PATH: &str = "object_storage.unsafe_path";
/// The download did not deliver the object's size.
pub const LENGTH_MISMATCH: &str = "object_storage.length_mismatch";
/// Anything else the service answered with.
pub const REQUEST_FAILED: &str = "object_storage.request_failed";
/// An upload could not be finished or verified.
pub const UPLOAD_FAILED: &str = "object_storage.upload_failed";
/// The service throttled the request or the account ran out of quota; retried later.
pub const RATE_LIMITED: &str = "object_storage.rate_limited";
/// The endpoint answered with a redirect, which is never followed (RD-1200-06).
pub const REDIRECT_REFUSED: &str = "object_storage.redirect_refused";

const fn transient() -> FailureKind {
    FailureKind::Transient {
        retry_after_seconds: None,
    }
}

/// Why no profile was chosen, as a queue failure.
#[must_use]
pub fn no_profile(choice: ProfileChoiceError, bucket: &str, hint: Option<&str>) -> Failure {
    match choice {
        ProfileChoiceError::None => Failure::coded(
            FailureKind::AuthRequired,
            NO_PROFILE,
            "No object storage profile serves this bucket",
        )
        .with_param("bucket", bucket),
        ProfileChoiceError::Ambiguous => Failure::coded(
            FailureKind::Permanent,
            PROFILE_AMBIGUOUS,
            "Several object storage profiles could serve this bucket; bind one to it",
        )
        .with_param("bucket", bucket),
        ProfileChoiceError::Disabled => Failure::coded(
            FailureKind::Permanent,
            PROFILE_DISABLED,
            "The object storage profile this link names is switched off",
        )
        .with_param("profile", hint.unwrap_or_default()),
    }
}

#[must_use]
pub fn address_invalid() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        ADDRESS_INVALID,
        "The address is not a valid object storage link",
    )
}

#[must_use]
pub fn object_changed() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        OBJECT_CHANGED,
        "The object changed since the partial download was written",
    )
}

#[must_use]
pub fn unsafe_path() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        UNSAFE_PATH,
        "The object name cannot be stored safely",
    )
}

/// Turns an `object_store` error into a coded failure.
#[must_use]
pub fn classify(error: &object_store::Error, bucket: &str) -> Failure {
    use object_store::Error;
    match error {
        Error::NotFound { .. } => not_found(bucket),
        Error::Precondition { .. } | Error::NotModified { .. } => object_changed(),
        Error::Unauthenticated { .. } => auth_failed(),
        // Google reports an exhausted quota with 403 too.
        Error::PermissionDenied { source, .. } if names_quota(&source.to_string()) => {
            rate_limited()
        }
        // S3 and Azure answer a wrong key or signature with 403 as well, so the service's
        // error code is what tells the two apart. Only that word is read, never passed on.
        Error::PermissionDenied { source, .. } if names_credential_error(&source.to_string()) => {
            auth_failed()
        }
        Error::PermissionDenied { .. } => access_denied(bucket),
        // Listing and the multipart calls report a refusal as a generic error that carries the
        // status in its text rather than as one of the variants above.
        Error::Generic { source, .. } => {
            let text = source.to_string();
            match status_in(&text) {
                Some(429) => rate_limited(),
                Some(503) if names_quota(&text) => rate_limited(),
                Some(403) if names_quota(&text) => rate_limited(),
                Some(401) => auth_failed(),
                Some(403) if names_credential_error(&text) => auth_failed(),
                // The token service of Azure or Google refused the service principal, the
                // service account key or the workload identity: `invalid_grant`,
                // `invalid_client` and their kin arrive as a 400.
                Some(400..=403) if text.contains("token request") => auth_failed(),
                Some(403) => access_denied(bucket),
                Some(404) => not_found(bucket),
                Some(412) => object_changed(),
                // The transport follows no redirect, so `object_store` reports the 3xx itself.
                Some(300..=399) => redirect_refused(),
                None if is_transport(&text) => Failure::coded(
                    transient(),
                    CONNECT_FAILED,
                    "The object storage endpoint could not be reached",
                ),
                _ => request_failed(),
            }
        }
        _ => request_failed(),
    }
}

fn not_found(bucket: &str) -> Failure {
    Failure::coded(
        FailureKind::Offline,
        NOT_FOUND,
        "The object or its bucket does not exist",
    )
    .with_param("bucket", bucket)
}

fn access_denied(bucket: &str) -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        ACCESS_DENIED,
        "The credentials may not access this object",
    )
    .with_param("bucket", bucket)
}

fn request_failed() -> Failure {
    Failure::coded(
        transient(),
        REQUEST_FAILED,
        "The object storage service refused the request",
    )
}

fn redirect_refused() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        REDIRECT_REFUSED,
        "The object storage endpoint answered with a redirect, which is not followed",
    )
}

/// The HTTP status `object_store` writes into the text of a refused request.
fn status_in(text: &str) -> Option<u16> {
    let (_, rest) = text.split_once("status code: ")?;
    rest.get(..3)?.parse().ok()
}

fn auth_failed() -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        AUTH_FAILED,
        "The object storage service refused the credentials",
    )
}

fn rate_limited() -> Failure {
    Failure::coded(
        FailureKind::RateLimited {
            retry_after_seconds: None,
        },
        RATE_LIMITED,
        "The object storage service throttled the request or the quota is exhausted",
    )
}

fn names_credential_error(text: &str) -> bool {
    [
        // S3
        "InvalidAccessKeyId",
        "SignatureDoesNotMatch",
        "ExpiredToken",
        "InvalidToken",
        "TokenRefreshRequired",
        // Azure: a wrong account key, an expired or tampered shared access signature.
        "AuthenticationFailed",
        "InvalidAuthenticationInfo",
    ]
    .iter()
    .any(|code| text.contains(code))
}

/// The throttling and quota codes: S3's `SlowDown`, Azure's `ServerBusy`, Google's
/// `rateLimitExceeded` and `quotaExceeded`.
fn names_quota(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "slowdown",
        "serverbusy",
        "ratelimitexceeded",
        "quotaexceeded",
    ]
    .iter()
    .any(|code| text.contains(code))
}

/// Whether an error without an HTTP status is the connection's rather than the service's.
fn is_transport(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "error sending request",
        "connect",
        "dns",
        "certificate",
        "tls",
        "timed out",
        "connection",
        "http error",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

#[cfg(test)]
mod tests {
    use rd_core::FailureKind;

    use super::{ACCESS_DENIED, AUTH_FAILED, NOT_FOUND, OBJECT_CHANGED, RATE_LIMITED, classify};

    fn boxed(text: &str) -> Box<dyn std::error::Error + Send + Sync> {
        text.to_owned().into()
    }

    #[test]
    fn a_wrong_key_is_told_apart_from_a_missing_permission() {
        let signature = object_store::Error::PermissionDenied {
            path: "a".to_owned(),
            source: boxed("<Code>SignatureDoesNotMatch</Code>"),
        };
        assert_eq!(
            classify(&signature, "media-bucket").code.as_deref(),
            Some(AUTH_FAILED)
        );
        let policy = object_store::Error::PermissionDenied {
            path: "a".to_owned(),
            source: boxed("<Code>AccessDenied</Code>"),
        };
        let failure = classify(&policy, "media-bucket");
        assert_eq!(failure.code.as_deref(), Some(ACCESS_DENIED));
        assert_eq!(
            failure.params.get("bucket").map(String::as_str),
            Some("media-bucket")
        );
    }

    #[test]
    fn a_failed_precondition_means_the_object_changed() {
        let changed = object_store::Error::Precondition {
            path: "a".to_owned(),
            source: boxed("412"),
        };
        let failure = classify(&changed, "b");
        assert_eq!(failure.code.as_deref(), Some(OBJECT_CHANGED));
        assert_eq!(failure.category, FailureKind::Permanent);
    }

    #[test]
    fn a_refused_listing_is_read_by_its_status() {
        let generic = |text: &str| object_store::Error::Generic {
            store: "S3",
            source: boxed(text),
        };
        let signature = generic(
            "Error performing GET http://127.0.0.1/b?list-type=2 in 2ms - Server returned \
             non-2xx status code: 403 Forbidden: <Error><Code>SignatureDoesNotMatch</Code></Error>",
        );
        assert_eq!(classify(&signature, "b").code.as_deref(), Some(AUTH_FAILED));
        let missing = generic("Server returned non-2xx status code: 404 Not Found: NoSuchBucket");
        assert_eq!(classify(&missing, "b").code.as_deref(), Some(NOT_FOUND));
        // A bucket named like a transport word must not turn a refusal into a network fault.
        let refused = generic(
            "Error performing GET http://connection-logs.example/ - Server returned non-2xx \
             status code: 400 Bad Request: InvalidArgument",
        );
        assert_eq!(
            classify(&refused, "b").code.as_deref(),
            Some(super::REQUEST_FAILED)
        );
        // RD-1200-06: the transport follows no redirect; the 3xx is named, not retried.
        let moved = generic(
            "Error performing GET http://127.0.0.1/b?list-type=2 in 1ms - Server returned \
             non-2xx status code: 307 Temporary Redirect: ",
        );
        let failure = classify(&moved, "b");
        assert_eq!(failure.code.as_deref(), Some(super::REDIRECT_REFUSED));
        assert_eq!(failure.category, FailureKind::Permanent);
        let down =
            generic("Error performing GET http://127.0.0.1/ - HTTP error: error sending request");
        assert_eq!(
            classify(&down, "b").code.as_deref(),
            Some(super::CONNECT_FAILED)
        );
    }

    #[test]
    fn azure_and_google_refusals_keep_their_meaning() {
        let generic = |text: &str| object_store::Error::Generic {
            store: "MicrosoftAzure",
            source: boxed(text),
        };
        // An expired shared access signature is a credential fault, not a missing permission.
        let expired = object_store::Error::PermissionDenied {
            path: "a".to_owned(),
            source: boxed(
                "Server returned non-2xx status code: 403 Forbidden: <Code>AuthenticationFailed\
                 </Code><AuthenticationErrorDetail>Signed expiry time has to be after signed \
                 start time</AuthenticationErrorDetail>",
            ),
        };
        assert_eq!(classify(&expired, "c").code.as_deref(), Some(AUTH_FAILED));
        let mismatch = object_store::Error::PermissionDenied {
            path: "a".to_owned(),
            source: boxed("<Code>AuthorizationPermissionMismatch</Code>"),
        };
        assert_eq!(
            classify(&mismatch, "c").code.as_deref(),
            Some(ACCESS_DENIED)
        );
        let token = generic(
            "Error performing token request: Server returned non-2xx status code: 400 Bad \
             Request: {\"error\":\"invalid_grant\"}",
        );
        assert_eq!(classify(&token, "c").code.as_deref(), Some(AUTH_FAILED));
    }

    #[test]
    fn throttling_and_quotas_are_retried_later_under_their_own_code() {
        let generic = |text: &str| object_store::Error::Generic {
            store: "GCS",
            source: boxed(text),
        };
        for text in [
            "Server returned non-2xx status code: 429 Too Many Requests: rateLimitExceeded",
            "Server returned non-2xx status code: 503 Service Unavailable: <Code>ServerBusy</Code>",
            "Server returned non-2xx status code: 503 Slow Down: <Code>SlowDown</Code>",
            "Server returned non-2xx status code: 403 Forbidden: quotaExceeded",
        ] {
            let failure = classify(&generic(text), "b");
            assert_eq!(failure.code.as_deref(), Some(RATE_LIMITED), "{text}");
            assert!(failure.category.is_retryable(), "{text}");
        }
        let quota = object_store::Error::PermissionDenied {
            path: "a".to_owned(),
            source: boxed("{\"reason\": \"quotaExceeded\"}"),
        };
        assert_eq!(classify(&quota, "b").code.as_deref(), Some(RATE_LIMITED));
        // A plain 503 stays an ordinary refusal.
        let unavailable = generic("Server returned non-2xx status code: 503 Service Unavailable");
        assert_eq!(
            classify(&unavailable, "b").code.as_deref(),
            Some(super::REQUEST_FAILED)
        );
    }

    #[test]
    fn the_service_text_never_reaches_the_failure() {
        let missing = object_store::Error::NotFound {
            path: "secret/path/name.bin".to_owned(),
            source: boxed("https://minio.example/media-bucket/secret/path/name.bin"),
        };
        let failure = classify(&missing, "media-bucket");
        assert_eq!(failure.code.as_deref(), Some(NOT_FOUND));
        let rendered = serde_json::to_string(&failure).expect("json");
        assert!(!rendered.contains("secret/path"), "{rendered}");
    }
}

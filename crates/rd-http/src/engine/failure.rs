//! How a response or a transport error becomes a classified [`Failure`].

use rd_core::{Failure, FailureKind};
use reqwest::{StatusCode, header};

use super::HttpDownloadError;

pub(super) fn header_text(
    response: &reqwest::Response,
    name: &header::HeaderName,
) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// A response that carries a page instead of the file.
///
/// Transient on purpose: a hoster's "please wait" notice is gone a minute later, and the
/// user already knows this wording from the online check.
pub(super) fn not_a_file(content_type: Option<String>) -> HttpDownloadError {
    let content_type = content_type.unwrap_or_else(|| "unknown".to_owned());
    let message = format!("The server returned {content_type} instead of the requested part");
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        "download.not_a_file",
        message,
    )
    .with_param("content_type", content_type)
    .into()
}

pub(crate) fn network_failure(error: reqwest::Error) -> HttpDownloadError {
    // A guarded client refused the address (RD-150-03): not a network that is down, and not
    // worth another attempt — the address will point at the same place next time.
    if crate::is_refusal(&error) {
        return Failure::coded(
            FailureKind::Permanent,
            rd_core::CODE_INTERNAL_ADDRESS,
            "The source points at an address a remote document may not reach",
        )
        .into();
    }
    let category = if error.is_connect() {
        FailureKind::Offline
    } else {
        FailureKind::Transient {
            retry_after_seconds: None,
        }
    };
    // `reqwest::Error`'s `Display` appends " for url (...)" with the fully expanded URL,
    // which for a presigned CDN link carries the signature in its query string. The message
    // is persisted in `downloads.last_error_json` and broadcast on SSE, so strip the URL and
    // redact whatever the remaining text still quotes.
    let message = rd_core::error_with_causes(&error.without_url());
    Failure::coded(category, "download.network_failed", message.clone())
        .with_param("detail", message)
        .into()
}

pub(crate) fn status_failure(status: StatusCode, headers: &header::HeaderMap) -> HttpDownloadError {
    let retry_after_seconds = headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        // The server's word, capped where it is read: everything downstream turns it into a
        // due time, and an unbounded one parked or panicked the download (audit 1.9.1, TR-01).
        .map(rd_core::clamp_retry_after);
    let category = match status.as_u16() {
        401 => FailureKind::AuthRequired,
        403 => FailureKind::AccountInvalid,
        404 | 410 => FailureKind::Permanent,
        429 => FailureKind::RateLimited {
            retry_after_seconds,
        },
        500..=599 => FailureKind::Transient {
            retry_after_seconds,
        },
        _ => FailureKind::Permanent,
    };
    Failure::coded(category, "download.http_status", format!("HTTP {status}"))
        .with_param("status", status)
        .into()
}

//! WebDAV transfer source (RD-060-03).
//!
//! WebDAV deliberately has **no** runner of its own. A WebDAV file is fetched with an
//! ordinary HTTP `GET`, so its queue row is a `DownloadKind::Http` row and it inherits the
//! existing engine's chunking, checkpoints, `Range`/`ETag` resume, auth profiles, proxy,
//! custom CA and bandwidth limit. What is WebDAV-specific is only the `PROPFIND` that turns
//! a link into a reviewable listing, which is what this crate provides.

mod propfind;

use anyhow::Result;
use rd_core::{Failure, FailureKind, RemoteListing, RemoteTarget};
use reqwest::{Client, StatusCode, header};
use url::Url;

pub use propfind::{MAX_BODY_BYTES, PROPFIND_BODY, ParseError, is_weak};

/// The server refused or could not answer the `PROPFIND`.
pub const PROPFIND_FAILED: &str = "webdav.propfind_failed";
/// The response was not a parseable `multistatus`.
pub const INVALID_RESPONSE: &str = "webdav.invalid_response";
/// A `href` in the response pointed outside the collection that was asked for.
pub const PATH_ESCAPES_ROOT: &str = "webdav.path_escapes_root";
/// The listing exceeded the size a single review can carry.
pub const LISTING_TOO_LARGE: &str = "webdav.listing_too_large";
/// The share needs credentials, or the ones supplied were rejected.
pub const AUTH_REQUIRED: &str = "webdav.auth_required";
/// The server does not offer byte ranges, so an interrupted download restarts.
pub const RANGE_UNSUPPORTED: &str = "webdav.range_unsupported";

/// What a link turned out to be.
pub enum Probed {
    Resolved(Box<RemoteListing>),
    Failed(Failure),
}

/// Issues `PROPFIND` with `Depth: 1` and turns the answer into a listing.
///
/// `client` is the same pooled client the transfer will use, so the probe authenticates
/// exactly the way the download does; building a separate one here would bypass the auth
/// profile rules, the proxy and the custom CA.
pub async fn probe(client: &Client, target: &RemoteTarget) -> Result<Probed> {
    let Some(url) = target.sanitized_url() else {
        return Ok(Probed::Failed(Failure::coded(
            FailureKind::Permanent,
            PROPFIND_FAILED,
            "The WebDAV address is not valid",
        )));
    };
    let response = client
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid method"),
            url.clone(),
        )
        // Depth 1 lists the collection and its direct children. Depth `infinity` is
        // refused by most servers and would hand back an unbounded body from the rest.
        .header("depth", "1")
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(PROPFIND_BODY)
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(_) => {
            return Ok(Probed::Failed(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                PROPFIND_FAILED,
                "The WebDAV server could not be reached",
            )));
        }
    };
    if let Some(failure) = status_failure(response.status()) {
        return Ok(Probed::Failed(failure));
    }
    // Range support is a property of `GET`, not of `PROPFIND`, but the header is commonly
    // present on both and is the only hint available before the transfer starts.
    let accepts_ranges = response
        .headers()
        .get(header::ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));

    let body = match bounded_text(response).await {
        Ok(body) => body,
        Err(failure) => return Ok(Probed::Failed(failure)),
    };
    match propfind::parse(&body, &url) {
        Ok(mut listing) => {
            listing.supports_resume = accepts_ranges;
            Ok(Probed::Resolved(Box::new(listing)))
        }
        // The offending href is not echoed back: it is server-controlled text that would
        // land in the UI and the logs.
        Err(ParseError::EscapesRoot { .. }) => Ok(Probed::Failed(Failure::coded(
            FailureKind::Permanent,
            PATH_ESCAPES_ROOT,
            "The WebDAV server listed a file outside the requested folder",
        ))),
        Err(ParseError::TooDeep | ParseError::Malformed) => Ok(Probed::Failed(Failure::coded(
            FailureKind::Permanent,
            INVALID_RESPONSE,
            "The WebDAV server did not return a usable directory listing",
        ))),
    }
}

/// Maps a non-success status onto a coded failure.
fn status_failure(status: StatusCode) -> Option<Failure> {
    if status.is_success() {
        return None;
    }
    Some(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Failure::coded(
            FailureKind::AuthRequired,
            AUTH_REQUIRED,
            "The WebDAV share needs credentials, or the stored ones were rejected",
        ),
        StatusCode::NOT_FOUND => Failure::coded(
            FailureKind::Permanent,
            PROPFIND_FAILED,
            "The WebDAV path does not exist",
        ),
        // 405 is what a plain HTTP server answers to PROPFIND: the address is reachable
        // but it is not a WebDAV share.
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED => Failure::coded(
            FailureKind::Permanent,
            PROPFIND_FAILED,
            "The server does not speak WebDAV at this address",
        ),
        status if status.is_server_error() => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            PROPFIND_FAILED,
            "The WebDAV server reported a temporary problem",
        )
        .with_param("status", status.as_u16()),
        status => Failure::coded(
            FailureKind::Permanent,
            PROPFIND_FAILED,
            "The WebDAV server refused the request",
        )
        .with_param("status", status.as_u16()),
    })
}

/// Reads the body, refusing anything above [`MAX_BODY_BYTES`].
///
/// The declared length is checked first so an oversized body is refused before a byte of it
/// is fetched. The body is then collected chunk by chunk and abandoned the moment it grows
/// past the limit, because the header is a claim and not a guarantee: a chunked response
/// declares no length at all, and a server that declares a small one is free to send
/// gigabytes anyway. Buffering the whole body first and measuring afterwards would let
/// either case stream unbounded data into memory, which is the one thing this function
/// exists to prevent. Same shape as `rd_http::probe::fetch_document`; `Response::chunk`
/// rather than `bytes_stream` only so this crate does not grow a `StreamExt` dependency
/// for a single loop.
async fn bounded_text(mut response: reqwest::Response) -> Result<String, Failure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return Err(too_large());
    }
    let unreadable = || {
        Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            PROPFIND_FAILED,
            "The WebDAV response could not be read",
        )
    };
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unreadable())? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_BODY_BYTES {
            return Err(too_large());
        }
    }
    String::from_utf8(bytes).map_err(|_| {
        Failure::coded(
            FailureKind::Permanent,
            INVALID_RESPONSE,
            "The WebDAV response was not valid UTF-8",
        )
    })
}

fn too_large() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        LISTING_TOO_LARGE,
        "The WebDAV directory listing is too large to review",
    )
    .with_param("limit", MAX_BODY_BYTES)
}

/// The `http(s)` URL a listing entry is downloaded from.
///
/// Built by joining the escaped path onto the collection URL rather than by string
/// concatenation, so a name containing spaces or `#` addresses the file it names.
#[must_use]
pub fn entry_url(base: &Url, relative_path: &str) -> Option<Url> {
    let mut url = base.clone();
    {
        let mut segments = url.path_segments_mut().ok()?;
        // A collection URL usually ends in `/`, leaving an empty final segment behind.
        segments.pop_if_empty();
        for segment in relative_path.split('/').filter(|part| !part.is_empty()) {
            segments.push(segment);
        }
    }
    Some(url)
}

/// The failure recorded on a candidate whose server cannot resume a download.
#[must_use]
pub fn range_unsupported() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        RANGE_UNSUPPORTED,
        "This WebDAV server cannot continue an interrupted download; it would start over",
    )
}

#[cfg(test)]
mod tests {
    use super::{AUTH_REQUIRED, PROPFIND_FAILED, entry_url, status_failure};
    use reqwest::StatusCode;
    use url::Url;

    fn url(input: &str) -> Url {
        Url::parse(input).expect("url")
    }

    #[test]
    fn entry_urls_escape_the_names_they_join() {
        // The bug this avoids: string concatenation would turn `a b.mkv` into a request
        // for `a` with a stray fragment, and `#1.mkv` into an empty path with a fragment.
        let base = url("https://cloud.example/dav/share/");
        assert_eq!(
            entry_url(&base, "my movie.mkv").expect("url").as_str(),
            "https://cloud.example/dav/share/my%20movie.mkv"
        );
        assert_eq!(
            entry_url(&base, "extras/#1 sample.mkv")
                .expect("url")
                .as_str(),
            "https://cloud.example/dav/share/extras/%231%20sample.mkv"
        );
    }

    #[test]
    fn a_collection_url_without_a_trailing_slash_still_joins() {
        assert_eq!(
            entry_url(&url("https://cloud.example/dav/share"), "a.bin")
                .expect("url")
                .as_str(),
            "https://cloud.example/dav/share/a.bin"
        );
    }

    #[test]
    fn an_unauthorised_share_asks_for_credentials() {
        let failure = status_failure(StatusCode::UNAUTHORIZED).expect("failure");
        assert_eq!(failure.code.as_deref(), Some(AUTH_REQUIRED));
        assert!(!failure.category.is_retryable());
    }

    #[test]
    fn a_plain_http_server_is_reported_as_not_webdav() {
        // 405 is exactly what a non-DAV server answers to PROPFIND.
        let failure = status_failure(StatusCode::METHOD_NOT_ALLOWED).expect("failure");
        assert_eq!(failure.code.as_deref(), Some(PROPFIND_FAILED));
        assert!(!failure.category.is_retryable());
    }

    #[test]
    fn a_server_error_is_retried_but_a_client_error_is_not() {
        assert!(
            status_failure(StatusCode::INTERNAL_SERVER_ERROR)
                .expect("failure")
                .category
                .is_retryable()
        );
        assert!(
            !status_failure(StatusCode::BAD_REQUEST)
                .expect("failure")
                .category
                .is_retryable()
        );
    }

    #[test]
    fn a_success_is_not_a_failure() {
        assert!(status_failure(StatusCode::MULTI_STATUS).is_none());
        assert!(status_failure(StatusCode::OK).is_none());
    }
}

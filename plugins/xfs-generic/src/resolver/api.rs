//! Request building and failure classification for the shared logic.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both targets.
//!
//! One thing differs from every site-specific plugin: there is no primary domain. A referer or a
//! link-host check has to be derived from the link being resolved, because which site this is
//! only becomes known when a URL arrives.

use plugin_common::{Failure, FailureKind, HttpRequest, HttpResponse};

use crate::messages;

/// A `GET` that asks for one byte, so a hotlink is recognised without downloading it.
pub(crate) fn range_probe(url: impl Into<String>) -> HttpRequest {
    HttpRequest::get(url).with_header("Range", "bytes=0-0")
}

pub(crate) fn is_html(response: &HttpResponse) -> bool {
    response
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"))
}

/// The XFS file code of a link, if the link is on a host this plugin claims.
///
/// The host list is consulted exactly, never by suffix. A suffix rule would claim
/// `xfs.example.org.attacker.test` and, worse, would make `matches()` answer yes for hosts the
/// sandbox never granted — which is the over-claiming the conformance check exists to catch.
pub(crate) fn file_code(url: &url::Url) -> Option<&str> {
    xfs_common::api::file_code(url, crate::HOSTERS)
}

/// Converts [`xfs_common::api::ErrorKind`]; used only by [`ensure_http_status`], since this
/// plugin never reads a JSON envelope.
fn convert_kind(kind: xfs_common::api::ErrorKind) -> FailureKind {
    match kind {
        xfs_common::api::ErrorKind::AccountInvalid => FailureKind::AccountInvalid,
        xfs_common::api::ErrorKind::Permanent => FailureKind::Permanent,
        xfs_common::api::ErrorKind::RateLimited => FailureKind::RateLimited(None),
        xfs_common::api::ErrorKind::Transient => FailureKind::Transient(None),
    }
}

pub(crate) fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
    match xfs_common::api::classify_http_status(response.status) {
        None => Ok(()),
        Some(kind) => Err(Failure::coded(
            convert_kind(kind),
            messages::HTTP_ERROR,
            messages::http_error(response.status),
        )
        .with_param("status", response.status.to_string())),
    }
}

pub(crate) fn invalid_url(error: &url::ParseError) -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        messages::INVALID_URL,
        messages::invalid_url(error),
    )
    .with_param("error", error.to_string())
}

pub(crate) fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

pub(crate) fn file_name_from_disposition(value: &str) -> Option<String> {
    value.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.eq_ignore_ascii_case("filename")
            .then(|| value.trim_matches(['\'', '"']).to_owned())
            .filter(|value| !value.is_empty())
    })
}

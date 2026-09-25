//! Request building, response parsing and failure classification for the shared logic.
//!
//! Mirrors `plugins/ddownload/src/resolver/api.rs` — KatFile runs the same XFileSharing engine —
//! with its own domains and JD's `rewriteHost` normalisation on top.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both.

use plugin_common::{Failure, FailureKind, HttpRequest, HttpResponse};
use serde::Deserialize;

use crate::messages;

/// The current live main domain: `KatfileCom.getPluginDomains()`'s index 0. Every API call and
/// the initial file-page fetch target this host, whichever alias the input link used.
pub(crate) const PRIMARY_DOMAIN: &str = "katfile.biz";
/// `https://katfile.biz/api` — KatFile serves its API from the main domain, unlike ddownload's
/// separate `api-v2.` host.
pub(crate) const API_BASE: &str = "https://katfile.biz/api";
pub(crate) const API_KEY_REFERENCE: &str = "katfile_api_key";

/// All seven domains JD's `getPluginDomains()` registers, each with its `www.` form.
pub(crate) const MATCH_HOSTS: &[&str] = &[
    "katfile.biz",
    "www.katfile.biz",
    "katfile.space",
    "www.katfile.space",
    "katfile.ws",
    "www.katfile.ws",
    "katfile.vip",
    "www.katfile.vip",
    "katfile.online",
    "www.katfile.online",
    "katfile.cloud",
    "www.katfile.cloud",
    "katfile.com",
    "www.katfile.com",
];

/// Rewrites a recognised alias to [`PRIMARY_DOMAIN`], mirroring JD's `KatfileCom.rewriteHost`:
/// KatFile's main domain has moved five times in a year, and a link on yesterday's alias has to
/// be browsed on today's.
pub(crate) fn canonicalize_host(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_owned();
    };
    if parsed
        .host_str()
        .is_some_and(|host| MATCH_HOSTS.contains(&host))
    {
        let _ = parsed.set_host(Some(PRIMARY_DOMAIN));
    }
    parsed.to_string()
}

/// A `GET` that asks for one byte, so a hotlink is recognised without downloading it.
pub(crate) fn range_probe(url: impl Into<String>) -> HttpRequest {
    HttpRequest::get(url).with_header("Range", "bytes=0-0")
}

pub(crate) fn is_html(response: &HttpResponse) -> bool {
    response
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"))
}

/// An API call carrying the `{{secret:…}}` marker the host expands. The key never enters here.
pub(crate) fn api_request(path: &str, extra: &[(&str, String)]) -> HttpRequest {
    let mut request = HttpRequest::get(format!("{API_BASE}/{path}"))
        .with_query("key", format!("{{{{secret:{API_KEY_REFERENCE}}}}}"));
    for (name, value) in extra {
        request = request.with_query(name, value.clone());
    }
    request
}

pub(crate) fn file_code(url: &url::Url) -> Option<&str> {
    xfs_common::api::file_code(url, MATCH_HOSTS)
}

pub(crate) fn file_name_from_disposition(value: &str) -> Option<String> {
    value.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.eq_ignore_ascii_case("filename")
            .then(|| value.trim_matches(['\'', '"']).to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// Converts [`xfs_common::api::ErrorKind`]; the classifier itself lives in `xfs-common`.
pub(crate) fn convert_kind(kind: xfs_common::api::ErrorKind) -> FailureKind {
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

pub(crate) fn convert_envelope_error(error: xfs_common::api::EnvelopeError) -> Failure {
    match error {
        xfs_common::api::EnvelopeError::Status(kind, message) => Failure::coded(
            convert_kind(kind),
            messages::API_ERROR,
            messages::api_error(&message),
        )
        .with_param("message", message),
        xfs_common::api::EnvelopeError::MissingResult => invalid_response(),
    }
}

pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(
    response: &HttpResponse,
) -> Result<T, Failure> {
    serde_json::from_slice(&response.body).map_err(|_| invalid_response())
}

pub(crate) fn invalid_response() -> Failure {
    coded(FailureKind::Transient(None), messages::INVALID_RESPONSE)
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

#[derive(Deserialize)]
pub(crate) struct AccountResult {
    pub(crate) email: String,
    pub(crate) premium_expire: String,
    pub(crate) traffic_left: Option<xfs_common::api::FlexibleU64>,
}

#[derive(Deserialize)]
pub(crate) struct DirectLink {
    pub(crate) url: String,
    pub(crate) size: Option<xfs_common::api::FlexibleU64>,
}

#[derive(Deserialize)]
pub(crate) struct FileInfo {
    pub(crate) status: u16,
    pub(crate) name: Option<String>,
    pub(crate) size: Option<xfs_common::api::FlexibleU64>,
}

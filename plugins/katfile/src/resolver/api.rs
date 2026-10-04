//! Request building, response parsing and failure classification for the shared logic.
//!
//! Mirrors `plugins/ddownload/src/resolver/api.rs` — KatFile runs the same XFileSharing engine —
//! with its own domains and JD's `rewriteHost` normalisation on top.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both.

use plugin_common::{Failure, HttpRequest, HttpResponse};
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

pub(crate) use xfs_common::glue::{coded, is_html, range_probe};

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

/// Classifies a transport status with the mapping every plugin shares, keeping a 429's
/// `Retry-After` (`xfs_common::glue`, RD-191-07).
pub(crate) fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
    xfs_common::glue::ensure_http_status(response, messages::HTTP_ERROR, messages::http_error)
}

/// The codes this plugin's JSON API failures are reported under.
const API_MESSAGES: xfs_common::glue::ApiMessages = xfs_common::glue::ApiMessages {
    api_error: messages::API_ERROR,
    api_error_text: messages::api_error,
    invalid_response: messages::INVALID_RESPONSE,
};

pub(crate) fn convert_envelope_error(error: xfs_common::api::EnvelopeError) -> Failure {
    API_MESSAGES.envelope_error(error)
}

pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(
    response: &HttpResponse,
) -> Result<T, Failure> {
    API_MESSAGES.parse_json(response)
}

pub(crate) fn invalid_url(error: &url::ParseError) -> Failure {
    xfs_common::glue::invalid_url(error, messages::INVALID_URL, messages::invalid_url)
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

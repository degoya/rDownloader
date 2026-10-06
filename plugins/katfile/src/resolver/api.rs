//! Request building, response parsing and failure classification for the shared logic.
//!
//! Mirrors `plugins/ddownload/src/resolver/api.rs` — KatFile runs the same XFileSharing engine —
//! with its own domains and JD's `rewriteHost` normalisation on top.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both.

use plugin_common::{Failure, HttpRequest, HttpResponse};

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

/// The codes this plugin's free flow reports its dead ends under (`xfs_common::free`,
/// RD-1110-03).
pub(crate) const FREE: xfs_common::free::FreeWords = xfs_common::free::FreeWords {
    http_error: plugin_common::HttpError {
        code: messages::HTTP_ERROR,
        text: messages::http_error,
    },
    no_free_form: (messages::NO_FREE_FORM, messages::no_free_form),
    no_free_link: (messages::NO_FREE_LINK, messages::no_free_link),
    free_limit_reached: (messages::FREE_LIMIT_REACHED, messages::free_limit_reached),
};

/// KatFile's documented API and cookie session, for the calls it shares with ddownload
/// (`xfs_common::site`, RD-1120-10).
pub(crate) const SITE: xfs_common::site::ApiSite = xfs_common::site::ApiSite {
    provider: "KatFile",
    primary_domain: PRIMARY_DOMAIN,
    api_request,
    http_error: plugin_common::HttpError {
        code: messages::HTTP_ERROR,
        text: messages::http_error,
    },
    api_messages: xfs_common::glue::ApiMessages {
        api_error: messages::API_ERROR,
        api_error_text: messages::api_error,
        invalid_response: messages::INVALID_RESPONSE,
    },
    file_unavailable: messages::FILE_UNAVAILABLE,
};

pub(crate) fn invalid_url(error: &url::ParseError) -> Failure {
    plugin_common::failure::invalid_url(messages::INVALID_URL, error).into()
}

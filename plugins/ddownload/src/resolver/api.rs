//! Request building, response parsing and failure classification for the shared logic.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both.

use plugin_common::{Failure, HttpRequest, HttpResponse};

use crate::messages;

/// ddownload's main domain — `DdownloadCom.getPluginDomains()`'s index 0 (the entry JD returns
/// from `Plugin.getHost()`; `ddl.to` and the CDN hosts are aliases). Used as the `Referer` the
/// free flow's final transfer must carry.
pub(crate) const PRIMARY_DOMAIN: &str = "ddownload.com";
pub(crate) const API_KEY_REFERENCE: &str = "ddownload_api_key";
/// The account password, in `login` credential mode. The host admits exactly one of the two
/// references per account, so probing which one it answers is also how this plugin learns
/// which mode the account is in.
pub(crate) const PASSWORD_REFERENCE: &str = "ddownload_password";

/// Hosts a ddownload link can carry, for [`xfs_common::api::file_code`].
pub(crate) const MATCH_HOSTS: &[&str] = &["ddownload.com", "www.ddownload.com"];

pub(crate) use xfs_common::glue::{coded, is_html, range_probe, set_cookies};

/// An API call carrying the `{{secret:…}}` marker the host expands. The key never enters here.
pub(crate) fn api_request(path: &str, extra: &[(&str, String)]) -> HttpRequest {
    api_request_with_key(path, &format!("{{{{secret:{API_KEY_REFERENCE}}}}}"), extra)
}

/// The same call with a key the plugin holds itself.
///
/// Only used in `login` credential mode, where there is no stored key to expand and the one in
/// hand was read off the signed-in account page. It is a value, not a marker, so it never
/// reaches the log: the host logs no request URLs, and this plugin logs none either.
pub(crate) fn api_request_with_key(path: &str, key: &str, extra: &[(&str, String)]) -> HttpRequest {
    let mut request = HttpRequest::get(format!("https://api-v2.ddownload.com/api/{path}"))
        .with_query("key", key.to_owned());
    for (name, value) in extra {
        request = request.with_query(name, value.clone());
    }
    request
}

/// The page carrying the sign-in form.
pub(crate) fn login_page_request() -> HttpRequest {
    HttpRequest::get(format!(
        "https://{PRIMARY_DOMAIN}{}",
        xfs_common::login::LOGIN_PATH
    ))
}

/// The signed-in account overview, which is where the API key is rendered.
pub(crate) fn account_page_request() -> HttpRequest {
    HttpRequest::get(format!(
        "https://{PRIMARY_DOMAIN}{}",
        xfs_common::login::ACCOUNT_INFO_PATH
    ))
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
/// RD-1110-03). The IP limit is read in the XFS base class's phrasings
/// (`xfs_common::free::ip_block_seconds`) unchanged: `DdownloadCom.checkErrors` adds only HTTP
/// 429/500 handling, already covered by [`ensure_http_status`], on top of `super.checkErrors`.
pub(crate) const FREE: xfs_common::free::FreeWords = xfs_common::free::FreeWords {
    http_error: plugin_common::HttpError {
        code: messages::HTTP_ERROR,
        text: messages::http_error,
    },
    no_free_form: (messages::NO_FREE_FORM, messages::no_free_form),
    no_free_link: (messages::NO_FREE_LINK, messages::no_free_link),
    free_limit_reached: (messages::FREE_LIMIT_REACHED, messages::free_limit_reached),
};

/// DDownload's documented API and cookie session, for the calls KatFile shares
/// (`xfs_common::site`, RD-1120-10).
pub(crate) const SITE: xfs_common::site::ApiSite = xfs_common::site::ApiSite {
    provider: "DDownload",
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

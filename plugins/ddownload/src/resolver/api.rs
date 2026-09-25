//! Request building, response parsing and failure classification for the shared logic.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both.

use plugin_common::{Failure, FailureKind, HttpRequest, HttpResponse};
use serde::Deserialize;

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

/// Every `Set-Cookie` value of a response, which `HttpResponse::header` cannot give: a sign-in
/// sets more than one and only one of them is the session.
pub(crate) fn set_cookies(response: &HttpResponse) -> Vec<String> {
    response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.clone())
        .collect()
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

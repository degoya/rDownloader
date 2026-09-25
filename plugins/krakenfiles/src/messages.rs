//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text. The
//! catalogue in `locales/*.json` carries the translations under the same codes.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The file page answered 404, or `/json/<id>` answered an empty array: deleted, or never there.
pub(crate) const FILE_UNAVAILABLE: (&str, &str) = (
    "krakenfiles.file_unavailable",
    "KrakenFiles file has been deleted or never existed",
);

/// The download form was posted with a Turnstile answer and the site said "captcha not valid"
/// twice in a row.
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "krakenfiles.captcha_rejected",
    "KrakenFiles rejected the captcha answer",
);

/// The download answer was `status: "error"` with a message other than the captcha refusal;
/// carries the site's `message`, trimmed and capped.
pub(crate) const DOWNLOAD_REFUSED: &str = "krakenfiles.download_refused";

/// The download answer was `status: "ok"` but carried no link.
pub(crate) const DIRECT_LINK_MISSING: (&str, &str) = (
    "krakenfiles.direct_link_missing",
    "KrakenFiles accepted the download request but returned no download link",
);

/// The link the site handed out lies outside the manifest's download domains; carries `host`.
pub(crate) const DIRECT_LINK_FOREIGN: &str = "krakenfiles.direct_link_foreign";

/// The direct link answered 403, 404 or 405 - JDownloader treats all three as "too many
/// connections, wait an hour"; carries `status`.
pub(crate) const RATE_LIMITED: &str = "krakenfiles.rate_limited";

/// The file page carried no usable download form; carries a `diagnosis` naming what is missing
/// or what page arrived instead.
pub(crate) const PAGE_LAYOUT_CHANGED: &str = "krakenfiles.page_layout_changed";

/// This provider takes no account, so there is never one to check. Reported rather than
/// silently succeeding: an account that appears valid but does nothing is worse than a clear
/// refusal.
pub(crate) const NO_ACCOUNT: (&str, &str) = (
    "krakenfiles.no_account",
    "This resolver downloads without an account and has none to check",
);

/// The URL is not a KrakenFiles file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "krakenfiles.unsupported_link",
    "Not a supported KrakenFiles link",
);

/// The URL could not be parsed.
pub(crate) const INVALID_LINK: (&str, &str) =
    ("krakenfiles.invalid_link", "Invalid KrakenFiles link");

/// The site answered with something that is neither the expected JSON nor the expected page.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "krakenfiles.invalid_response",
    "Invalid KrakenFiles response",
);

/// Unexpected HTTP status; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "krakenfiles.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "krakenfiles.invalid_url";

pub(crate) fn download_refused(message: &str) -> String {
    format!("KrakenFiles refused the download: {message}")
}

pub(crate) fn direct_link_foreign(host: &str) -> String {
    format!(
        "KrakenFiles handed out a download link on {host}, which this plugin may not download from"
    )
}

pub(crate) fn rate_limited(status: u16) -> String {
    format!(
        "KrakenFiles refused the download link (HTTP {status}); too many connections, try again in an hour"
    )
}

pub(crate) fn page_layout_changed(diagnosis: &str) -> String {
    format!("The KrakenFiles file page no longer looks as expected: {diagnosis}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("KrakenFiles HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

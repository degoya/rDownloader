//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text, and
//! `locales/` translates exactly these codes. Nothing MediaFire wrote appears verbatim in any
//! of them: a provider message travels as a sanitised `message` parameter.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The URL is not a MediaFire file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "mediafire.unsupported_link",
    "Not a supported MediaFire link",
);

/// The URL could not be parsed, or the API said the key has no valid shape (error 111).
pub(crate) const INVALID_LINK: (&str, &str) = ("mediafire.invalid_link", "Invalid MediaFire link");

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "mediafire.invalid_url";

/// The API or the file page answered with something that is not the expected document.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("mediafire.invalid_response", "Invalid MediaFire response");

/// Unexpected HTTP status; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "mediafire.http_error";

/// The API envelope reported an error this plugin has no closer name for; carries the
/// sanitised provider `message`.
pub(crate) const API_ERROR: &str = "mediafire.api_error";

/// API error 261: the per-resource call limit was reached.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "mediafire.rate_limited",
    "MediaFire is rate limiting API calls from this address",
);

/// API error 110, or the file page redirected to `errno=320`: the file is gone.
pub(crate) const FILE_UNAVAILABLE: (&str, &str) = (
    "mediafire.file_unavailable",
    "MediaFire file is not available",
);

/// The upload is still being processed (`ready: no`).
pub(crate) const FILE_NOT_READY: (&str, &str) = (
    "mediafire.file_not_ready",
    "MediaFire is still processing this file",
);

/// The site blocked the file; carries a `reason` naming which of its error pages said so.
pub(crate) const FILE_BLOCKED: &str = "mediafire.file_blocked";

/// `privacy: private`, or error page 999: only the owner may read it, and this plugin signs
/// nobody in.
pub(crate) const PRIVATE_FILE: (&str, &str) = (
    "mediafire.private_file",
    "MediaFire file is private and needs the owner's account",
);

/// `password_protected: yes`: this plugin does not answer file passwords.
pub(crate) const PASSWORD_REQUIRED: (&str, &str) = (
    "mediafire.password_required",
    "MediaFire file is password protected",
);

/// Error page 394: the uploader's own limit; carries a `reason`.
pub(crate) const OWNER_LIMIT: &str = "mediafire.owner_limit";

/// The page carries the malware advisory; the file is not downloaded.
pub(crate) const MALWARE_FLAGGED: (&str, &str) = (
    "mediafire.malware_flagged",
    "MediaFire flagged this file as malware",
);

/// `limitReachedTTL`: this address may not start another download yet; carries `wait_seconds`.
pub(crate) const DOWNLOAD_LIMIT_REACHED: &str = "mediafire.download_limit_reached";

/// The site asked for a short wait; carries `wait_seconds`.
pub(crate) const TEMPORARILY_UNAVAILABLE: &str = "mediafire.temporarily_unavailable";

/// The page carries a captcha form this plugin cannot hand over.
pub(crate) const CAPTCHA_REQUIRED: (&str, &str) = (
    "mediafire.captcha_required",
    "MediaFire asks for a captcha this plugin cannot answer",
);

/// The site rejected the captcha answer.
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "mediafire.captcha_rejected",
    "MediaFire rejected the captcha answer",
);

/// The file page carried no direct link; carries a `diagnosis`. Never the page address.
pub(crate) const NO_DIRECT_LINK: &str = "mediafire.no_direct_link";

/// A folder address was handed to the file resolver.
pub(crate) const FOLDER_NOT_FILE: (&str, &str) = (
    "mediafire.folder_not_file",
    "This is a MediaFire folder, not a file; folders are listed by the MediaFire folder crawler",
);

/// An error page with a number this plugin does not know; carries `errno`.
pub(crate) const ERROR_PAGE: &str = "mediafire.error_page";

/// This provider takes no account.
pub(crate) const NO_ACCOUNT: (&str, &str) = (
    "mediafire.no_account",
    "MediaFire resolves public files without an account; there is nothing to check",
);

pub(crate) fn http_error(status: u16) -> String {
    format!("MediaFire HTTP status {status}")
}

pub(crate) fn api_error(message: &str) -> String {
    format!("MediaFire API: {message}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

pub(crate) fn file_blocked(reason: &str) -> String {
    format!("MediaFire blocked this file: {reason}")
}

pub(crate) fn owner_limit(reason: &str) -> String {
    format!("MediaFire refuses the download because of the uploader's limit: {reason}")
}

pub(crate) fn download_limit_reached(seconds: u64) -> String {
    format!("MediaFire download threshold exceeded for this address; try again in {seconds}s")
}

pub(crate) fn temporarily_unavailable(seconds: u64) -> String {
    format!("MediaFire download is temporarily unavailable; try again in {seconds}s")
}

pub(crate) fn no_direct_link(diagnosis: &str) -> String {
    format!("MediaFire file page carried no direct link: {diagnosis}")
}

pub(crate) fn error_page(errno: u32) -> String {
    format!("MediaFire answered with error page {errno}")
}

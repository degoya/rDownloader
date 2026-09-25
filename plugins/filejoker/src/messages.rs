//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text. Mirrors
//! `plugins/ddownload/src/messages.rs`'s taxonomy (`filejoker.*` in place of `ddownload.*`), minus
//! the API-key-only codes ddownload/katfile carry (no API-key path exists here — see
//! `native/api.rs`'s module doc), plus [`SESSION_INVALID`]/[`FILE_OFFLINE`]/[`PAGE_ERROR`], which
//! this plugin's task brief calls for explicitly (a distinct login-wall code, a page-marker-based
//! offline code, and a catch-all unknown-page code), and [`CAPTCHA_REQUIRED`], mirroring
//! `plugins/katfile/src/messages.rs`'s addition of the same.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The account has no cookie session at all (the only credential this provider uses). Checked
/// before any authenticated request on both `check_account` and `resolve`.
pub(crate) const COOKIES_MISSING: (&str, &str) = (
    "filejoker.cookies_missing",
    "FileJoker cookie session is missing or contains no cookies for filejoker.net",
);

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("filejoker.account_missing", "FileJoker account is missing");

/// The URL is not a FileJoker file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "filejoker.unsupported_link",
    "Not a supported FileJoker link",
);

/// The URL could not be parsed (guest adapter only; the native host already hands over a parsed
/// `Url`).
pub(crate) const INVALID_LINK: (&str, &str) = ("filejoker.invalid_link", "Invalid FileJoker link");

/// Unexpected HTTP status; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "filejoker.http_error";

/// A provider URL failed to parse (native adapter only); carries the parser `error`.
pub(crate) const INVALID_URL: &str = "filejoker.invalid_url";

/// The `download2` premium form (or its containing file page) carries a captcha challenge
/// (reCaptcha/hCaptcha/Cloudflare Turnstile) this plugin has no way to solve. Mirrors
/// `plugins/katfile/src/messages.rs::CAPTCHA_REQUIRED`; unlike KatFile, this is not a JD citation
/// (no JD reference exists) but is corroborated by `pyload`'s FileJoker GitHub issues, which
/// report captcha challenges on this flow repeatedly changing shape over time (see
/// `native/api.rs`'s module doc).
pub(crate) const CAPTCHA_REQUIRED: (&str, &str) = (
    "filejoker.captcha_required",
    "FileJoker requires solving a captcha for this download, which is not supported",
);

/// The file page carries FileJoker's file-not-found marker. Carries no `diagnosis` parameter —
/// unlike the other page-classification codes below, this one is a specific, positively-matched
/// marker rather than a generic fallback, so the raw page text adds nothing.
pub(crate) const FILE_OFFLINE: (&str, &str) =
    ("filejoker.file_offline", "FileJoker file is not available");

/// The cookie session's request landed on a login page instead of the expected content — the
/// cookies were not sent, are stale, or do not belong to a logged-in session. Carries a
/// `diagnosis` parameter (`xfs_common::page::diagnose`'s explanatory text).
pub(crate) const SESSION_INVALID: &str = "filejoker.session_invalid";

/// The site answered with a page that is neither signed in nor a guest page, so the session
/// could not be confirmed either way. Carries a `diagnosis` parameter. Not an account fault:
/// reported as transient, because FileJoker's pages are unmeasured and an unrecognized page is
/// a gap in this code, not a broken account (RD-108-28 review).
pub(crate) const SESSION_UNCONFIRMED: &str = "filejoker.session_unconfirmed";

/// The file page (or the response after posting the premium form) reports the file requires a
/// premium account. Carries a `diagnosis` parameter.
pub(crate) const NO_PREMIUM_FILE: &str = "filejoker.no_premium_file";

/// A response that is not a login wall, a premium-only notice, an offline marker or a wait
/// countdown, yet still is not the file itself (no `download2` form, or no direct link found after
/// posting one) — an unrecognized page shape. Carries a `diagnosis` parameter.
pub(crate) const PAGE_ERROR: &str = "filejoker.page_error";

/// The file page reports a pre-download wait before the file becomes downloadable. Carries the
/// `wait_seconds` parameter.
pub(crate) const DOWNLOAD_WAIT: &str = "filejoker.download_wait";

/// The free flow found no `download1`/`download2` form to work with; carries a `diagnosis`.
pub(crate) const NO_FREE_FORM: &str = "filejoker.no_free_form";

/// The free flow reached its last step but the page carried no direct link; carries a
/// `diagnosis`.
pub(crate) const NO_FREE_LINK: &str = "filejoker.no_free_link";

/// This IP may not start another free download yet; carries `wait_seconds` when the page stated
/// one. Mirrors `plowshare`'s `Please wait ... until the next download` handling — see
/// `crate::page`'s module doc.
pub(crate) const FREE_LIMIT_REACHED: &str = "filejoker.free_limit_reached";

/// The file is too large for FileJoker's free tier (`plowshare`'s `Free user can't download
/// large files`, reported there as `ERR_LINK_NEED_PERMISSIONS`): no wait or captcha will make
/// this link downloadable without an account.
pub(crate) const FREE_SIZE_LIMIT: (&str, &str) = (
    "filejoker.free_size_limit",
    "FileJoker does not allow free downloads of a file this large",
);

/// The hoster rejected the captcha answer even after a fresh challenge.
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "filejoker.captcha_rejected",
    "FileJoker rejected the captcha answer",
);

/// `check()` is not implemented (FileJoker has no documented link-status API; see
/// `native/api.rs`'s module doc). Deliberately the same generic `(code, message)` pair the native
/// `Resolver` trait's own `check()` default reports (`crates/rd-plugin-api/src/lib.rs`), so the
/// guest adapter — which must implement every WIT export explicitly, unlike the native trait,
/// which has a default — reproduces it byte-for-byte instead of drifting from a hand-copied
/// literal. Mirrors `plugins/debridlink/src/messages.rs::CHECK_UNSUPPORTED`.
pub(crate) const CHECK_UNSUPPORTED: (&str, &str) = (
    "link.check_unsupported",
    "Link check is not supported by this resolver",
);

pub(crate) fn http_error(status: u16) -> String {
    format!("FileJoker HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

pub(crate) fn session_invalid(diagnosis: &str) -> String {
    format!("FileJoker cookie session requires logging in again: {diagnosis}")
}

pub(crate) fn session_unconfirmed(diagnosis: &str) -> String {
    format!(
        "FileJoker did not confirm the cookie session either way - the site answered with a \
         page that is neither signed in nor a guest page: {diagnosis}"
    )
}

pub(crate) fn no_premium_file(diagnosis: &str) -> String {
    format!("FileJoker reports this file requires a premium account: {diagnosis}")
}

pub(crate) fn page_error(diagnosis: &str) -> String {
    format!("FileJoker returned an unrecognized page: {diagnosis}")
}

pub(crate) fn download_wait(seconds: u64) -> String {
    format!("FileJoker requires waiting {seconds}s before this file can be downloaded")
}

pub(crate) fn no_free_form(diagnosis: &str) -> String {
    format!("FileJoker free download form was not found: {diagnosis}")
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("FileJoker free download did not yield a file link: {diagnosis}")
}

pub(crate) fn free_limit_reached(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => format!(
            "FileJoker free download limit reached; another download is possible in {seconds}s"
        ),
        None => "FileJoker free download limit reached for this IP address".to_owned(),
    }
}

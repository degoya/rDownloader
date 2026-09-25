//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text. Mirrors
//! `plugins/ddownload/src/messages.rs`'s taxonomy (`katfile.*` in place of `ddownload.*`), plus
//! [`CAPTCHA_REQUIRED`] — ddownload has no equivalent, see `native/api.rs`'s module doc.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// `file/info` reported a status other than 200 for the file.
pub(crate) const FILE_UNAVAILABLE: (&str, &str) =
    ("katfile.file_unavailable", "KatFile file is not available");

/// The account has neither an API key nor an imported cookie session.
pub(crate) const COOKIE_SESSION_REQUIRED: (&str, &str) = (
    "katfile.cookie_session_required",
    "KatFile cookie session is missing or contains no cookies for katfile.biz",
);

/// The API key alone cannot download; the premium flow needs browser cookies.
pub(crate) const COOKIE_SESSION_REQUIRED_FOR_DOWNLOAD: (&str, &str) = (
    "katfile.cookie_session_required_for_download",
    "KatFile downloads require a cookie session (browser login); the API key only provides metadata",
);

/// The imported cookie session reached the site but is not signed in; carries a `diagnosis`.
///
/// Until RD-120-13 this branch had no code at all, because it had no verdict to report: any
/// 2xx answer counted as proof of a session, and an expired one is served the guest homepage
/// with status 200.
pub(crate) const COOKIE_SESSION_INVALID: &str = "katfile.cookie_session_invalid";

/// The site answered with a page that is neither signed in nor a guest page, so the session
/// could not be confirmed either way; carries a `diagnosis`. Not an account fault: reported as
/// transient, the same rule ddownload's check follows.
pub(crate) const COOKIE_SESSION_UNCONFIRMED: &str = "katfile.cookie_session_unconfirmed";

/// The API key answered for the account, but the cookie session a download runs on has lapsed;
/// carries a `diagnosis`. Kept apart from [`COOKIE_SESSION_INVALID`]: there the session is the
/// account, here the account is proven and only the browser session has to be replaced.
pub(crate) const DOWNLOAD_SESSION_EXPIRED: &str = "katfile.download_session_expired";

/// A label part, not a failure: the API key proved the account, and the page asked about the
/// cookie session settled nothing (RD-120-46). Until then this was reported as
/// [`COOKIE_SESSION_UNCONFIRMED`] and failed the whole check over an account the key had just
/// proven — the fault RD-120-44 fixed in ddownload.
pub(crate) const SESSION_UNCONFIRMED: (&str, &str) = (
    "katfile.session_unconfirmed",
    "cookie session not confirmed - the page was neither signed in nor a guest page",
);

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("katfile.account_missing", "KatFile account is missing");

/// The URL is not a KatFile file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) =
    ("katfile.unsupported_link", "Not a supported KatFile link");

/// The URL could not be parsed.
pub(crate) const INVALID_LINK: (&str, &str) = ("katfile.invalid_link", "Invalid KatFile link");

/// Link checks go through the metadata API and therefore need the API key.
pub(crate) const API_KEY_REQUIRED: (&str, &str) = (
    "katfile.api_key_required",
    "KatFile link check requires the API key",
);

/// The API or the file page answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("katfile.invalid_response", "Invalid KatFile response");

/// The cookie session returned an HTML page instead of the file; carries a `diagnosis`.
pub(crate) const NO_PREMIUM_FILE: &str = "katfile.no_premium_file";

/// Unexpected HTTP status; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "katfile.http_error";

/// The API envelope reported an error; carries the provider `message`.
pub(crate) const API_ERROR: &str = "katfile.api_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "katfile.invalid_url";

/// The premium `download2` form (or its containing file page) carries a captcha challenge
/// (reCaptchaV2/hCaptcha/Cloudflare Turnstile) that this plugin has no way to solve. Mirrors JD's
/// `KatfileCom.findFormDownload2Premium` -> `XFileSharingProBasic.handleCaptcha` — see
/// `native/api.rs`'s module doc for the full citation; ddownload's JD plugin has no equivalent
/// override, so this code has no counterpart in `plugins/ddownload`.
pub(crate) const CAPTCHA_REQUIRED: (&str, &str) = (
    "katfile.captcha_required",
    "KatFile requires solving a captcha for this download, which is not supported",
);

/// The file page (or the response after posting the premium form) reports the file is
/// premium-only. Mirrors JD's `KatfileCom.getPremiumOnlyErrorMessage` override
/// (`">\s*This file is available for Premium"` / a `/?op=registration&redirect=` URL); carries
/// the matched `reason` text. See `native/api.rs`'s module doc.
pub(crate) const PREMIUM_ONLY: &str = "katfile.premium_only";

/// The file page reports a pre-download wait before the file becomes downloadable. Mirrors JD's
/// `KatfileCom.regexWaittime` override (`var estimated_time = (\d+)`, in tenths of a second);
/// carries the `wait_seconds` parameter. See `native/api.rs`'s module doc.
pub(crate) const DOWNLOAD_WAIT: &str = "katfile.download_wait";

/// The free flow found no `download1`/`download2` form to work with; carries a `diagnosis`.
pub(crate) const NO_FREE_FORM: &str = "katfile.no_free_form";

/// The free flow reached its last step but the page carried no direct link; carries a
/// `diagnosis`.
pub(crate) const NO_FREE_LINK: &str = "katfile.no_free_link";

/// This IP may not start another free download yet; carries `wait_seconds` when the page
/// stated one. Mirrors JD's `ERROR_IP_BLOCKED` handling in `XFileSharingProBasic.checkErrors`.
pub(crate) const FREE_LIMIT_REACHED: &str = "katfile.free_limit_reached";

/// The hoster rejected the captcha answer even after a fresh challenge.
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "katfile.captcha_rejected",
    "KatFile rejected the captcha answer",
);

pub(crate) fn no_free_form(diagnosis: &str) -> String {
    format!("KatFile free download form was not found: {diagnosis}")
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("KatFile free download did not yield a file link: {diagnosis}")
}

pub(crate) fn free_limit_reached(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => {
            format!(
                "KatFile free download limit reached; another download is possible in {seconds}s"
            )
        }
        None => "KatFile free download limit reached for this IP address".to_owned(),
    }
}

pub(crate) fn cookie_session_invalid(diagnosis: &str) -> String {
    format!(
        "KatFile cookie session is not signed in - paste a fresh cookie session from a \
         signed-in browser: {diagnosis}"
    )
}

pub(crate) fn cookie_session_unconfirmed(diagnosis: &str) -> String {
    format!(
        "KatFile did not confirm the cookie session either way - the site answered with a page \
         that is neither signed in nor a guest page: {diagnosis}"
    )
}

pub(crate) fn download_session_expired(diagnosis: &str) -> String {
    format!(
        "KatFile accepted the API key, but the cookie session downloads run on has expired - \
         sign in at katfile.biz in a browser and paste a fresh cookie session into this \
         account: {diagnosis}"
    )
}

pub(crate) fn no_premium_file(diagnosis: &str) -> String {
    format!("KatFile cookie session did not return a premium file: {diagnosis}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("KatFile HTTP status {status}")
}

pub(crate) fn api_error(message: &str) -> String {
    format!("KatFile API: {message}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

pub(crate) fn premium_only(reason: &str) -> String {
    format!("KatFile reports this file requires a premium account: {reason}")
}

pub(crate) fn download_wait(seconds: u64) -> String {
    format!("KatFile requires waiting {seconds}s before this file can be downloaded")
}

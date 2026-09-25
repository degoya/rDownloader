//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// `file/info` reported a status other than 200 for the file.
pub(crate) const FILE_UNAVAILABLE: (&str, &str) = (
    "ddownload.file_unavailable",
    "DDownload file is not available",
);

/// The account has neither an API key nor an imported cookie session.
pub(crate) const COOKIE_SESSION_REQUIRED: (&str, &str) = (
    "ddownload.cookie_session_required",
    "DDownload cookie session is missing or contains no cookies for ddownload.com",
);

/// The imported cookie session reached the site but is not signed in; carries a `diagnosis`.
pub(crate) const COOKIE_SESSION_INVALID: &str = "ddownload.cookie_session_invalid";

/// The site answered with a page that is neither signed in nor a guest page, so the session
/// could not be confirmed either way; carries a `diagnosis`. Not an account fault: reported as
/// transient so the account keeps working while the site serves an interstitial, a maintenance
/// notice or a layout this code does not recognize (RD-108-28 review).
pub(crate) const COOKIE_SESSION_UNCONFIRMED: &str = "ddownload.cookie_session_unconfirmed";

/// The API key answered for the account, but the cookie session a download runs on has lapsed;
/// carries a `diagnosis`.
///
/// Kept apart from [`COOKIE_SESSION_INVALID`] on purpose (RD-120-13). There the session *is*
/// the account and the account is what failed; here the account is proven and only the browser
/// session has to be replaced, which is a different thing to tell somebody to do.
pub(crate) const DOWNLOAD_SESSION_EXPIRED: &str = "ddownload.download_session_expired";

/// A label part, not a failure: the API key proved the account, and the account page settled
/// nothing about the cookie session a download runs on (RD-120-44). Until then this was
/// reported as [`COOKIE_SESSION_UNCONFIRMED`] and failed the whole check over an account the
/// key had just proven.
pub(crate) const SESSION_UNCONFIRMED: (&str, &str) = (
    "ddownload.session_unconfirmed",
    "cookie session not confirmed - the account page was neither signed in nor a guest page",
);

/// The API key alone cannot download; the premium flow needs browser cookies.
pub(crate) const COOKIE_SESSION_REQUIRED_FOR_DOWNLOAD: (&str, &str) = (
    "ddownload.cookie_session_required_for_download",
    "DDownload downloads require a cookie session (browser login); the API key only provides metadata",
);

/// The account can neither sign in nor present an imported cookie session.
pub(crate) const LOGIN_CREDENTIALS_REQUIRED: (&str, &str) = (
    "ddownload.login_credentials_required",
    "DDownload downloads need either account credentials or an imported cookie session",
);

/// The site said the stored username or password is wrong.
pub(crate) const LOGIN_FAILED: (&str, &str) = (
    "ddownload.login_failed",
    "DDownload rejected the account's username or password",
);

/// The site refused this IP rather than the credentials, so a later attempt may work.
pub(crate) const LOGIN_BLOCKED: (&str, &str) = (
    "ddownload.login_blocked",
    "DDownload refused the sign-in from this IP address",
);

/// The login page did not carry the sign-in form; carries a `diagnosis` naming the page that
/// arrived instead.
pub(crate) const LOGIN_FORM_MISSING: &str = "ddownload.login_form_missing";

/// The sign-in was neither confirmed nor explained; carries a `diagnosis`.
pub(crate) const LOGIN_UNAVAILABLE: &str = "ddownload.login_unavailable";

/// The login form is behind a browser challenge, so the credentials were never read.
///
/// DDownload put Cloudflare Turnstile on its login form after this plugin's sign-in shipped.
/// A resolver has no browser and cannot produce a token, so signing in with a stored password
/// cannot complete at all — and the honest thing is to say which challenge and what still
/// works, rather than report a failure the user could spend an evening on. Carries `challenge`.
pub(crate) const LOGIN_CAPTCHA: &str = "ddownload.login_captcha";

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("ddownload.account_missing", "DDownload account is missing");

/// The URL is not a DDownload file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "ddownload.unsupported_link",
    "Not a supported DDownload link",
);

/// The URL could not be parsed.
pub(crate) const INVALID_LINK: (&str, &str) = ("ddownload.invalid_link", "Invalid DDownload link");

/// Link checks go through the metadata API and therefore need the API key.
pub(crate) const API_KEY_REQUIRED: (&str, &str) = (
    "ddownload.api_key_required",
    "DDownload link check requires the API key",
);

/// The API or the file page answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("ddownload.invalid_response", "Invalid DDownload response");

/// The cookie session returned an HTML page instead of the file; carries a `diagnosis`.
pub(crate) const NO_PREMIUM_FILE: &str = "ddownload.no_premium_file";

/// Unexpected HTTP status; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "ddownload.http_error";

/// The API envelope reported an error; carries the provider `message`.
pub(crate) const API_ERROR: &str = "ddownload.api_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "ddownload.invalid_url";

/// The free flow found no `download1`/`download2` form to work with; carries a `diagnosis`.
pub(crate) const NO_FREE_FORM: &str = "ddownload.no_free_form";

/// The free flow reached its last step but the page carried no direct link; carries a
/// `diagnosis`.
pub(crate) const NO_FREE_LINK: &str = "ddownload.no_free_link";

/// This IP may not start another free download yet; carries `wait_seconds` when the page stated
/// one. Mirrors the `ERROR_IP_BLOCKED` handling in JD's `XFileSharingProBasic.checkErrors`, which
/// `DdownloadCom.checkErrors` delegates to unchanged (it only adds HTTP 429/500 handling).
pub(crate) const FREE_LIMIT_REACHED: &str = "ddownload.free_limit_reached";

/// The hoster rejected the captcha answer even after a fresh challenge.
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "ddownload.captcha_rejected",
    "DDownload rejected the captcha answer",
);

pub(crate) fn no_free_form(diagnosis: &str) -> String {
    format!("DDownload free download form was not found: {diagnosis}")
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("DDownload free download did not yield a file link: {diagnosis}")
}

pub(crate) fn free_limit_reached(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => format!(
            "DDownload free download limit reached; another download is possible in {seconds}s"
        ),
        None => "DDownload free download limit reached for this IP address".to_owned(),
    }
}

pub(crate) fn cookie_session_invalid(diagnosis: &str) -> String {
    format!(
        "DDownload cookie session is not signed in - paste a fresh cookie session from a \
         signed-in browser: {diagnosis}"
    )
}

pub(crate) fn download_session_expired(diagnosis: &str) -> String {
    format!(
        "DDownload accepted the API key, but the cookie session downloads run on has expired - \
         sign in at ddownload.com in a browser and paste a fresh cookie session into this \
         account: {diagnosis}"
    )
}

pub(crate) fn cookie_session_unconfirmed(diagnosis: &str) -> String {
    format!(
        "DDownload did not confirm the cookie session either way - the site answered with a \
         page that is neither signed in nor a guest page: {diagnosis}"
    )
}

pub(crate) fn no_premium_file(diagnosis: &str) -> String {
    format!("DDownload cookie session did not return a premium file: {diagnosis}")
}

pub(crate) fn login_unavailable(diagnosis: &str) -> String {
    format!("DDownload did not confirm the sign-in: {diagnosis}")
}

/// English fallback for the captcha refusal; the catalogue carries the translations.
pub(crate) fn login_captcha(challenge: &str) -> String {
    format!(
        "DDownload's login form is protected by {challenge}, which rDownloader cannot answer. \
         Switch this account to API key mode, or paste a cookie session from a signed-in browser."
    )
}

/// English fallback for the missing sign-in form; the catalogue carries the translations.
pub(crate) fn login_form_missing(diagnosis: &str) -> String {
    format!("DDownload's login page did not contain the expected sign-in form: {diagnosis}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("DDownload HTTP status {status}")
}

pub(crate) fn api_error(message: &str) -> String {
    format!("DDownload API: {message}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

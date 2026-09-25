//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
//! The taxonomy is `plugins/filejoker/src/messages.rs`'s free-flow half, minus everything that
//! implies an account (no cookie session, no premium notice, no login wall to report) and plus
//! [`NO_ACCOUNT`], which this plugin needs because the host may still ask a resolver to check an
//! account and this one has none to check.
//!
//! The texts name "this XFileSharing site" rather than a hoster, because the whole point is that
//! the plugin does not know which clone it is talking to. Where a clone deviates from the
//! standard flow, [`NO_FREE_FORM`] and [`NO_FREE_LINK`] carry a `diagnosis` taken from the page
//! itself, so the failure names a cause instead of being an empty answer.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The URL is not on a domain this plugin claims. While the domain list is empty this is the
/// answer for every link, which is exactly what an inert plugin should say.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "xfs_generic.unsupported_link",
    "Not a link on an XFileSharing site this plugin serves",
);

/// The URL could not be parsed (guest adapter only; the native host already hands over a parsed
/// `Url`).
pub(crate) const INVALID_LINK: (&str, &str) =
    ("xfs_generic.invalid_link", "Invalid XFileSharing link");

/// This provider takes no account, so there is never one to check. Reported rather than silently
/// succeeding: an account that appears valid but does nothing is worse than a clear refusal.
pub(crate) const NO_ACCOUNT: (&str, &str) = (
    "xfs_generic.no_account",
    "This resolver downloads without an account and has none to check",
);

/// Unexpected HTTP status; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "xfs_generic.http_error";

/// A provider URL failed to parse (native adapter only); carries the parser `error`.
pub(crate) const INVALID_URL: &str = "xfs_generic.invalid_url";

/// The free flow found no `download1`/`download2` form to work with — the usual shape of a clone
/// that deviates from the standard script. Carries a `diagnosis`.
pub(crate) const NO_FREE_FORM: &str = "xfs_generic.no_free_form";

/// The free flow reached its last step but the page carried no direct link; carries a
/// `diagnosis`.
pub(crate) const NO_FREE_LINK: &str = "xfs_generic.no_free_link";

/// This IP may not start another free download yet; carries `wait_seconds` when the page stated
/// one.
pub(crate) const FREE_LIMIT_REACHED: &str = "xfs_generic.free_limit_reached";

/// The site rejected the captcha answer even after a fresh challenge.
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "xfs_generic.captcha_rejected",
    "The XFileSharing site rejected the captcha answer",
);

/// `check()` is not implemented: XFS exposes a link-status API only to an account, which is
/// precisely what this plugin does not have. Deliberately the same generic `(code, message)`
/// pair the native `Resolver` trait's own `check()` default reports, so the guest adapter — which
/// must implement every WIT export explicitly — reproduces it rather than drifting from a
/// hand-copied literal.
pub(crate) const CHECK_UNSUPPORTED: (&str, &str) = (
    "link.check_unsupported",
    "Link check is not supported by this resolver",
);

pub(crate) fn http_error(status: u16) -> String {
    format!("XFileSharing site answered HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

pub(crate) fn no_free_form(diagnosis: &str) -> String {
    format!("No XFileSharing free download form was found on the page: {diagnosis}")
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("The XFileSharing free download did not yield a file link: {diagnosis}")
}

pub(crate) fn free_limit_reached(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => format!(
            "Free download limit reached on this XFileSharing site; another download is possible in {seconds}s"
        ),
        None => {
            "Free download limit reached on this XFileSharing site for this IP address".to_owned()
        }
    }
}

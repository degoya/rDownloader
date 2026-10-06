//! Thin wrapper binding `xfs-common`'s generalized XFS page helpers to KatFile's own
//! parameterization, plus two KatFile-specific parsers (`premium_only_reason`,
//! `estimated_wait_seconds`) not generalized into `xfs-common` since they mirror JD overrides
//! specific to `KatfileCom` — see the crate's module doc. No KatFile-specific deviation was
//! found for the `op` marker or the premium button label (JD's `KatfileCom` does not override
//! `findFormDownload2Premium`'s field values, only wraps it with a captcha check), so the forms
//! are the script's own (`xfs_common::standard`, re-exported here); only the direct-link domain
//! differs.

const DOMAIN: &str = "katfile.biz";

pub(crate) use xfs_common::page::encode_form;
pub(crate) use xfs_common::standard::{download2_form as download_form, free_form, premium_form};

/// Seconds to wait before the free download may be requested. KatFile's own
/// `var estimated_time` marker (tenths of a second) is tried first, then the XFS base
/// class's generic countdown markers.
#[must_use]
pub(crate) fn free_wait_seconds(html: &str) -> Option<u64> {
    estimated_wait_seconds(html).or_else(|| xfs_common::free::countdown_seconds(html))
}

/// The raw `<form>...</form>` substring carrying `op=download2`, for scoping a captcha-marker
/// scan to the form itself rather than the whole page (see the crate's module doc).
#[must_use]
pub(crate) fn form_html(html: &str) -> Option<&str> {
    xfs_common::page::form_html(html, xfs_common::standard::OP_DOWNLOAD2)
}

/// Explains why an HTML page came back instead of a file, for error messages.
#[must_use]
pub(crate) fn diagnose(html: &str) -> String {
    xfs_common::page::diagnose(html)
}

pub(crate) use xfs_common::page::SessionState;

/// What a page says about the visitor's session, with the diagnosis of that same page.
///
/// KatFile's account check used to accept any 2xx answer as proof of a cookie session, which
/// is no proof at all: an expired session is served the guest homepage with status 200. The
/// rule is the shared one ddownload and FileJoker already use, with its three answers kept
/// apart — the sign-out link is a session, the site's own guest markup is not, and a page
/// carrying neither settles nothing (RD-120-13).
#[must_use]
pub(crate) fn classify_session(html: &str) -> SessionState {
    xfs_common::page::classify_session(html)
}

/// Finds the premium direct link on the page returned after submitting the form.
#[must_use]
pub(crate) fn direct_link(html: &str, hints: &[&str]) -> Option<String> {
    xfs_common::page::direct_link(html, hints, DOMAIN)
}

/// Whether `html` (the `download2` form's own HTML, scoped via [`form_html`] — see
/// the crate's module doc) shows a captcha challenge this plugin cannot solve.
#[must_use]
pub(crate) fn has_captcha_challenge(html: &str) -> bool {
    xfs_common::page::has_captcha_challenge(html)
}

/// KatFile's `getPremiumOnlyErrorMessage` additions (JD `KatfileCom.java:309-317`, on top of the
/// XFS base class's own generic phrase list, which this plugin does not otherwise model — see
/// the crate's module doc): a distinct "This file is available for Premium" phrasing, and a
/// `/?op=registration&redirect=` URL marker. Returns the reason text for the failure message;
/// `None` if neither marker is found.
#[must_use]
pub(crate) fn premium_only_reason(html: &str, final_url: &str) -> Option<&'static str> {
    if html.contains("This file is available for Premium") {
        Some("This file is available for Premium")
    } else if final_url.contains("/?op=registration&redirect=") {
        Some("Account required to download this file")
    } else {
        None
    }
}

/// KatFile's `regexWaittime` override (JD `KatfileCom.java:320-328`): `var estimated_time =
/// (\d+)` counts TENTHS of a second, not seconds — JD's own comment: "Small hack: These aren't
/// seconds but tenths of a second". `None` if the marker is absent, unparseable, or rounds to
/// zero seconds. See the crate's module doc.
#[must_use]
pub(crate) fn estimated_wait_seconds(html: &str) -> Option<u64> {
    const MARKER: &str = "var estimated_time";
    let after = &html[html.find(MARKER)? + MARKER.len()..];
    let after = after.trim_start().strip_prefix('=')?.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    let tenths: u64 = digits.parse().ok()?;
    let seconds = tenths / 10;
    (seconds > 0).then_some(seconds)
}

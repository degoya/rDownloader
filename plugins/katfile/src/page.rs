//! Thin wrapper binding `xfs-common`'s generalized XFS page helpers to KatFile's own
//! parameterization, plus two KatFile-specific parsers (`premium_only_reason`,
//! `estimated_wait_seconds`) not generalized into `xfs-common` since they mirror JD overrides
//! specific to `KatfileCom` — see `native/api.rs`'s module doc. No KatFile-specific deviation was
//! found for the `op` marker or the premium button label (JD's `KatfileCom` does not override
//! `findFormDownload2Premium`'s field values, only wraps it with a captcha check), so these match
//! ddownload's defaults; only the direct-link/premium-only domain differs.

const OP_DOWNLOAD1: &str = "download1";
const OP_DOWNLOAD2: &str = "download2";
const PREMIUM_BUTTON: &str = "Premium Download";
const FREE_BUTTON: &str = "Free Download";
const DOMAIN: &str = "katfile.biz";

/// Hidden form fields of the `download1` form, the first step of the free flow.
#[must_use]
pub(crate) fn download1_form(html: &str) -> Option<Vec<(String, String)>> {
    xfs_common::page::download_form(html, OP_DOWNLOAD1)
}

/// Turns raw form fields into the free submission (keeps `method_free`, drops the premium
/// marker) — the counterpart of [`premium_form`].
#[must_use]
pub(crate) fn free_form(fields: &[(String, String)]) -> Vec<(String, String)> {
    xfs_common::free::free_form(fields, FREE_BUTTON)
}

/// The captcha widget a page asks for, with the site key needed to solve it.
#[must_use]
pub(crate) fn widget_marker(html: &str) -> Option<xfs_common::free::WidgetMarker> {
    xfs_common::free::widget_marker(html)
}

/// Adds a solved captcha's token to a form under its widget's field name.
#[must_use]
pub(crate) fn with_captcha_token(
    fields: &[(String, String)],
    kind: xfs_common::free::WidgetKind,
    token: &str,
) -> Vec<(String, String)> {
    xfs_common::free::with_captcha_token(fields, kind, token)
}

/// Seconds to wait before the free download may be requested. KatFile's own
/// `var estimated_time` marker (tenths of a second) is tried first, then the XFS base
/// class's generic countdown markers.
#[must_use]
pub(crate) fn free_wait_seconds(html: &str) -> Option<u64> {
    estimated_wait_seconds(html).or_else(|| xfs_common::free::countdown_seconds(html))
}

/// Seconds this IP must wait for another free download, or `Some(0)` when the page states a
/// limit without naming a duration.
#[must_use]
pub(crate) fn ip_block_seconds(html: &str) -> Option<u64> {
    xfs_common::free::ip_block_seconds(html)
}

/// Whether the page says the captcha answer was rejected.
#[must_use]
pub(crate) fn is_wrong_captcha(html: &str) -> bool {
    xfs_common::free::is_wrong_captcha(html)
}

/// Hidden form fields of the `download2` form on a file page.
#[must_use]
pub(crate) fn download_form(html: &str) -> Option<Vec<(String, String)>> {
    xfs_common::page::download_form(html, OP_DOWNLOAD2)
}

/// The raw `<form>...</form>` substring carrying `op=download2`, for scoping a captcha-marker
/// scan to the form itself rather than the whole page (see `native/api.rs`'s module doc).
#[must_use]
pub(crate) fn form_html(html: &str) -> Option<&str> {
    xfs_common::page::form_html(html, OP_DOWNLOAD2)
}

/// Turns the raw `download2` fields into the premium submission JDownloader sends.
#[must_use]
pub(crate) fn premium_form(fields: &[(String, String)]) -> Vec<(String, String)> {
    xfs_common::page::premium_form(fields, PREMIUM_BUTTON)
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

/// Encodes form fields as `application/x-www-form-urlencoded`.
#[must_use]
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    xfs_common::page::encode_form(fields)
}

/// Finds the premium direct link on the page returned after submitting the form.
#[must_use]
pub(crate) fn direct_link(html: &str, hints: &[&str]) -> Option<String> {
    xfs_common::page::direct_link(html, hints, DOMAIN)
}

/// Whether `html` (the `download2` form's own HTML, scoped via [`form_html`] — see
/// `native/api.rs`'s module doc) shows a captcha challenge this plugin cannot solve.
#[must_use]
pub(crate) fn has_captcha_challenge(html: &str) -> bool {
    xfs_common::page::has_captcha_challenge(html)
}

/// KatFile's `getPremiumOnlyErrorMessage` additions (JD `KatfileCom.java:309-317`, on top of the
/// XFS base class's own generic phrase list, which this plugin does not otherwise model — see
/// `native/api.rs`'s module doc): a distinct "This file is available for Premium" phrasing, and a
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
/// zero seconds. See `native/api.rs`'s module doc.
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

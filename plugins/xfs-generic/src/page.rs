//! The XFileSharing page rules, bound to the script's own defaults and nothing else.
//!
//! Every function here is a thin wrapper over `xfs-common`. That is the point: where
//! `plugins/filejoker/src/page.rs` and `plugins/katfile/src/page.rs` add markers their own site
//! phrases differently, this plugin adds none. A clone whose pages need one of those overrides
//! is not a clone this plugin serves — it belongs in a plugin of its own, and here it fails with
//! a named cause rather than being guessed at.
//!
//! The constants below are the XFS base script's defaults, the same values `ddownload` and
//! `katfile` use: `op=download1` for the first step of the free flow, `op=download2` for the
//! second, and `Free Download` as the free button's label.

const OP_DOWNLOAD1: &str = "download1";
const OP_DOWNLOAD2: &str = "download2";
/// The XFS base script's own fallback label.
const FREE_BUTTON: &str = "Free Download";

/// Hidden form fields of the `download1` form, the first step of the free flow.
#[must_use]
pub(crate) fn download1_form(html: &str) -> Option<Vec<(String, String)>> {
    xfs_common::page::download_form(html, OP_DOWNLOAD1)
}

/// Hidden form fields of the `download2` form, the second step.
#[must_use]
pub(crate) fn download2_form(html: &str) -> Option<Vec<(String, String)>> {
    xfs_common::page::download_form(html, OP_DOWNLOAD2)
}

/// Turns raw form fields into the free submission: drops the premium marker and sets
/// `method_free` to the label the site expects.
///
/// The one place this plugin cannot use a constant the way a site-specific plugin does.
/// `xfs_common::free::free_form` overwrites `method_free` with the label it is given, which is
/// right when the label is known — `ddownload` and `katfile` each know their own. A generic
/// resolver does not: clones word that button differently ("Slow Download", "Download Now", a
/// translation), and a site that checks the value would reject a submission carrying someone
/// else's wording. So the page's own value wins when it has one, and [`FREE_BUTTON`] is the
/// fallback for a form that carries the field empty or not at all.
#[must_use]
pub(crate) fn free_form(fields: &[(String, String)]) -> Vec<(String, String)> {
    let button = fields
        .iter()
        .find(|(name, _)| name == "method_free")
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(FREE_BUTTON);
    xfs_common::free::free_form(fields, button)
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

/// Seconds to wait before the free download may be requested.
#[must_use]
pub(crate) fn free_wait_seconds(html: &str) -> Option<u64> {
    xfs_common::free::countdown_seconds(html)
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

/// The direct link on the final page, recognised by the hints the link itself carries.
#[must_use]
pub(crate) fn direct_link(html: &str, hints: &[&str], domains: &[&str]) -> Option<String> {
    xfs_common::page::direct_link_any(html, hints, domains)
}

/// Form fields encoded for an `application/x-www-form-urlencoded` post.
#[must_use]
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    xfs_common::page::encode_form(fields)
}

/// A short explanation of what a page appears to be, for a failure that would otherwise be
/// empty.
#[must_use]
pub(crate) fn diagnose(html: &str) -> String {
    xfs_common::page::diagnose(html)
}

#[cfg(test)]
#[path = "page/tests.rs"]
mod tests;

//! Thin wrapper binding `xfs-common`'s generalized XFS page helpers to FileJoker's own
//! parameterization, plus FileJoker-specific parsers not generalized into `xfs-common` (mirrors
//! `plugins/katfile/src/page.rs`'s pattern for `KatfileCom`-specific overrides, adapted here since
//! no JD source exists at all — see `native/api.rs`'s module doc for the verification record
//! behind every marker below).

//!
//! The account-less (free) flow's wrappers were added on top of that, mirroring
//! `plugins/katfile/src/page.rs`. IMPL-VERIFY (still **no JD reference**: `FileJoker*.java` is
//! absent from `mycodedoesnotcompile2/jdownloader_mirror`'s `svn_trunk` — re-confirmed 2026-09-02
//! for `FileJokerNet.java`, `FilejokerNet.java` and `FileJoker.java`, all 404), verified instead
//! against `mcrapet/plowshare-modules-legacy`'s `filejoker.sh`, which implements the free flow in
//! full (`filejoker_download`, the branch taken when the account is not premium):
//! - **Two-step form flow**: the file page carries a first form (named `F22`) with
//!   `op`/`usr_login`/`id`/`fname`/`referer`/`method_free`, posted to the file URL; the answer
//!   page carries a second form (named `F1`) with
//!   `op`/`id`/`rand`/`referer`/`method_free`/`method_premium`/`down_direct`, posted to the same
//!   URL. That is exactly the XFS two-form shape this plugin's free flow drives.
//! - **Countdown**: `Please Wait <tag>N</tag> seconds` — whole seconds, already modeled by
//!   [`estimated_wait_seconds`], which [`free_wait_seconds`] tries before the XFS base class's
//!   generic markers.
//! - **IP limit**: `Please wait .* until the next download`, whose hours/minutes/seconds
//!   plowshare sums into one delay — modeled by [`forced_delay_seconds`], which
//!   [`ip_block_seconds`] tries before the base class's own phrasings.
//! - **Free size limit**: `Free user can't download large files` — modeled by
//!   [`is_free_size_limited`].
//! - **Rejected captcha**: `Wrong Captcha`, already one of
//!   `xfs_common::free::is_wrong_captcha`'s markers.
//! - **Not verified**: the `op` values. plowshare reads `op` dynamically off each form rather
//!   than asserting a literal, so [`OP_DOWNLOAD1`]/[`OP_DOWNLOAD2`] stay the XFS defaults
//!   ddownload and KatFile use — if FileJoker's values differ, the forms are simply not found
//!   and the flow reports `filejoker.no_free_form` rather than posting something wrong. The free
//!   button label is likewise the XFS default (`"Free Download"`), and only used when the page
//!   itself carries no `method_free` value to echo back. plowshare posts both forms as
//!   `multipart/form-data` (`curl -F`) where this plugin posts
//!   `application/x-www-form-urlencoded`, as the rest of the XFS family does; PHP accepts either,
//!   but this was not confirmed against FileJoker itself. Finally, plowshare's captcha handling
//!   dates from 2016/2017 and still looks for reCAPTCHA v1's `recaptcha_challenge_field`; the
//!   current widget is unknown, so the free flow uses the generic
//!   `xfs_common::free::widget_marker`, which recognizes reCaptchaV2, hCaptcha and Turnstile.

const OP_DOWNLOAD1: &str = "download1";
const OP_DOWNLOAD2: &str = "download2";
const PREMIUM_BUTTON: &str = "Premium Download";
/// The XFS base class's own fallback label — see the module doc.
const FREE_BUTTON: &str = "Free Download";
const DOMAIN: &str = "filejoker.net";

/// Hidden form fields of the `download2` form on a file page.
#[must_use]
pub(crate) fn download_form(html: &str) -> Option<Vec<(String, String)>> {
    xfs_common::page::download_form(html, OP_DOWNLOAD2)
}

/// Hidden form fields of the `download1` form, the first step of the free flow.
#[must_use]
pub(crate) fn download1_form(html: &str) -> Option<Vec<(String, String)>> {
    xfs_common::page::download_form(html, OP_DOWNLOAD1)
}

/// The raw `<form>...</form>` substring carrying `op=download2`, for scoping a captcha-marker
/// scan to the form itself rather than the whole page.
#[must_use]
pub(crate) fn form_html(html: &str) -> Option<&str> {
    xfs_common::page::form_html(html, OP_DOWNLOAD2)
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

/// Seconds to wait before the free download may be requested. FileJoker's own `Please Wait
/// <tag>N</tag> seconds` marker is tried first, then the XFS base class's generic countdown
/// markers — see the module doc.
#[must_use]
pub(crate) fn free_wait_seconds(html: &str) -> Option<u64> {
    estimated_wait_seconds(html).or_else(|| xfs_common::free::countdown_seconds(html))
}

/// Seconds this IP must wait for another free download, or `Some(0)` when the page states a
/// limit without naming a duration. FileJoker's own forced-delay phrasing is tried first, then
/// the XFS base class's — see the module doc.
#[must_use]
pub(crate) fn ip_block_seconds(html: &str) -> Option<u64> {
    forced_delay_seconds(html).or_else(|| xfs_common::free::ip_block_seconds(html))
}

/// Whether the page says the captcha answer was rejected.
#[must_use]
pub(crate) fn is_wrong_captcha(html: &str) -> bool {
    xfs_common::free::is_wrong_captcha(html)
}

/// FileJoker's forced-delay notice between two free downloads (`plowshare`'s `filejoker.sh`:
/// `matchi 'Please wait .* until the next download'`, whose hour/minute/second groups it sums
/// into one delay). Deliberately requires the trailing `until the next download` phrase: the
/// pre-download countdown ([`estimated_wait_seconds`]) opens with the same two words and must not
/// be mistaken for an IP block. `None` if the notice is absent; `Some(0)` if it is present
/// without a parseable duration, which still has to hold the hoster back.
#[must_use]
fn forced_delay_seconds(html: &str) -> Option<u64> {
    const TAIL: &str = "until the next download";
    // plowshare matches the sentence case-insensitively; only the two spellings the page can
    // realistically use are checked here, so the capital-W countdown ("Please Wait ") cannot
    // reach this parser even when the rest of the sentence happens to follow it.
    let start = ["Please wait ", "please wait "]
        .into_iter()
        .filter_map(|marker| html.find(marker).map(|at| at + marker.len()))
        .min()?;
    let after = &html[start..];
    // Bounded so an unrelated, far-away occurrence of the tail phrase cannot be pulled in;
    // `get` keeps the cut panic-free when byte 160 falls inside a multi-byte character.
    let window = after.get(..160).unwrap_or(after);
    let end = window.find(TAIL)?;
    Some(xfs_common::free::unit_sum_seconds(&window[..end]).unwrap_or(0))
}

/// FileJoker's free-tier size limit (`plowshare`'s `filejoker.sh`: `match "Free user can't
/// download large files"`, reported there as `ERR_LINK_NEED_PERMISSIONS`) — no wait or captcha
/// will ever make this file downloadable without an account.
#[must_use]
pub(crate) fn is_free_size_limited(html: &str) -> bool {
    html.contains("Free user can't download large files")
}

/// Turns the raw `download2` fields into the premium submission this plugin sends.
#[must_use]
pub(crate) fn premium_form(fields: &[(String, String)]) -> Vec<(String, String)> {
    xfs_common::page::premium_form(fields, PREMIUM_BUTTON)
}

/// Explains why an HTML page came back instead of a file, for error messages.
#[must_use]
pub(crate) fn diagnose(html: &str) -> String {
    xfs_common::page::diagnose(html)
}

/// Encodes form fields as `application/x-www-form-urlencoded`.
#[must_use]
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    xfs_common::page::encode_form(fields)
}

/// Finds the premium direct link on the page returned after submitting the form. Uses
/// `xfs-common`'s own-domain heuristic (matching ddownload/katfile) rather than the anchor-*text*
/// match ("Download File") the `plowshare` reference module uses — see `native/api.rs`'s module
/// doc's open-question note on this.
#[must_use]
pub(crate) fn direct_link(html: &str, hints: &[&str]) -> Option<String> {
    xfs_common::page::direct_link(html, hints, DOMAIN)
}

/// Whether `html` (expected to be scoped via [`form_html`]) shows a captcha challenge this plugin
/// cannot solve.
#[must_use]
pub(crate) fn has_captcha_challenge(html: &str) -> bool {
    xfs_common::page::has_captcha_challenge(html)
}

/// Whether `html` shows the same login-wall markers `xfs_common::page::diagnose` checks first (its
/// `LoginModal` script ships on every page, logged in or not; only the navigation login link and
/// the login form mark an anonymous visitor). Exposed by `xfs-common` itself (promoted there after
/// review — this plugin used to hand-copy the marker literals here, which risked drifting from
/// `diagnose`'s own check), so callers can distinguish "this specific page is a login wall" (->
/// `filejoker.session_invalid`) from `diagnose`'s other, more generic fallback text (->
/// `filejoker.no_premium_file` / `filejoker.page_error`) without duplicating any matching logic.
#[must_use]
pub(crate) fn is_login_wall(html: &str) -> bool {
    xfs_common::page::is_login_wall(html)
}

pub(crate) use xfs_common::page::SessionState;

/// What the page says about the visitor's session, with the diagnosis of that same page.
///
/// The three-state answer, not the bare `shows_signed_in` predicate this wrapper once
/// forwarded: the predicate says whether the sign-out link is there, which is not the same
/// question as whether the page is evidence about the account at all. Collapsing the two was
/// the RD-108-28 review's finding — a page carrying neither marker is unrecognized, not a
/// broken account. Shared with the other XFS plugins since RD-120-13, so none of them reads
/// the diagnosis off a different page than the one it judged.
#[must_use]
pub(crate) fn classify_session(html: &str) -> SessionState {
    xfs_common::page::classify_session(html)
}

/// FileJoker's file-not-found marker (`mcrapet/plowshare-modules-legacy`'s `filejoker.sh`: `match
/// 'File Not Found' "$PAGE"`) — see `native/api.rs`'s module doc.
#[must_use]
pub(crate) fn is_file_offline(html: &str) -> bool {
    html.contains("File Not Found")
}

/// FileJoker's premium-only marker (`mcrapet/plowshare-modules-legacy`'s `filejoker.sh`: `match
/// '<div class="premium-download-expand">' "$PAGE"`) — see `native/api.rs`'s module doc.
#[must_use]
pub(crate) fn is_premium_only(html: &str) -> bool {
    html.contains(r#"<div class="premium-download-expand">"#)
}

/// FileJoker's pre-download wait marker: `Please Wait <tag>N</tag> seconds` (`plowshare`'s
/// `filejoker.sh`: `parse_quiet 'Please Wait ' 'Wait <.\+>\([[:digit:]]\+\)<.\+> seconds'`).
/// Unlike KatFile's `var estimated_time = N` marker (tenths of a second), this is already whole
/// seconds — see `native/api.rs`'s module doc. Anchored on the more specific `"Please Wait "`
/// (review finding: a bare `"Wait "` marker could match a stray, unrelated occurrence of the word
/// elsewhere on the page first). `None` if the marker is absent or unparseable.
#[must_use]
pub(crate) fn estimated_wait_seconds(html: &str) -> Option<u64> {
    const MARKER: &str = "Please Wait ";
    let after_wait = &html[html.find(MARKER)? + MARKER.len()..];
    let after_open_tag = &after_wait[after_wait.find('>')? + 1..];
    let digits: String = after_open_tag
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let after_digits = &after_open_tag[digits.len()..];
    let after_close_tag = &after_digits[after_digits.find('>')? + 1..];
    after_close_tag
        .trim_start()
        .starts_with("seconds")
        .then(|| digits.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::{
        diagnose, direct_link, download_form, download1_form, encode_form, estimated_wait_seconds,
        form_html, free_form, free_wait_seconds, has_captcha_challenge, ip_block_seconds,
        is_file_offline, is_free_size_limited, is_login_wall, is_premium_only, premium_form,
    };

    const PAGE: &str = r#"<html><body>
<form name="F1" method="POST" action="">
  <input type="hidden" name="op" value="download2">
  <input type="hidden" name="id" value="abc123xyz">
  <input type="hidden" name="rand" value="r4nd">
  <input type="hidden" name="fname" value="release.rar">
  <input type="hidden" name="referer" value="">
  <input type="hidden" name="method_free" value="">
  <input type="hidden" name="method_premium" value="">
  <input type="hidden" name="down_direct" value="1">
</form></body></html>"#;

    #[test]
    fn extracts_hidden_fields_of_the_download_form() {
        let fields = download_form(PAGE).expect("form");
        assert_eq!(
            fields,
            [
                ("op", "download2"),
                ("id", "abc123xyz"),
                ("rand", "r4nd"),
                ("fname", "release.rar"),
                ("referer", ""),
                ("method_free", ""),
                ("method_premium", ""),
                ("down_direct", "1"),
            ]
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
        );
        assert!(!encode_form(&fields).is_empty());
        assert!(download_form("<html><form><input name='x'></form></html>").is_none());
    }

    #[test]
    fn form_html_scopes_to_the_form_carrying_the_op_marker() {
        let outside = format!(r#"<div class="g-recaptcha" data-sitekey="unrelated"></div>{PAGE}"#);
        let scoped = form_html(&outside).expect("form html");
        assert!(scoped.starts_with("<form"));
        assert!(!has_captcha_challenge(scoped));
        assert!(has_captcha_challenge(&outside), "sanity: marker exists");
    }

    #[test]
    fn premium_form_drops_free_marker_and_sets_premium_button() {
        let form = download_form(PAGE).expect("form");
        let premium = premium_form(&form);
        assert!(premium.iter().all(|(name, _)| name != "method_free"));
        assert_eq!(
            premium.iter().find(|(name, _)| name == "method_premium"),
            Some(&("method_premium".to_owned(), "Premium Download".to_owned()))
        );
    }

    /// plowshare's first free form (`F22`), whose fields this plugin reaches through the XFS
    /// default `op` marker — see the module doc's "not verified" note.
    const DOWNLOAD1_PAGE: &str = r#"<form name="F22" method="POST" action="">
  <input type="hidden" name="op" value="download1">
  <input type="hidden" name="usr_login" value="">
  <input type="hidden" name="id" value="abc123xyz">
  <input type="hidden" name="fname" value="release.rar">
  <input type="hidden" name="referer" value="">
  <input type="hidden" name="method_free" value="Free Download">
  <input type="hidden" name="method_premium" value="">
</form>"#;

    #[test]
    fn free_form_keeps_the_free_marker_and_drops_the_premium_one() {
        let fields = download1_form(DOWNLOAD1_PAGE).expect("download1 form");
        assert!(download_form(DOWNLOAD1_PAGE).is_none(), "not download2");
        let free = free_form(&fields);
        assert!(free.iter().all(|(name, _)| name != "method_premium"));
        assert_eq!(
            free.iter().find(|(name, _)| name == "method_free"),
            Some(&("method_free".to_owned(), "Free Download".to_owned()))
        );
        assert!(free.iter().any(|(name, _)| name == "usr_login"));
    }

    /// The pre-download countdown and the between-downloads forced delay open with the same two
    /// words; only the latter is an IP block, and only it carries the "until the next download"
    /// tail (`plowshare`'s two distinct patterns).
    #[test]
    fn the_countdown_and_the_forced_delay_are_told_apart() {
        let countdown = "<p>Please Wait <b>45</b> seconds</p>";
        assert_eq!(free_wait_seconds(countdown), Some(45));
        assert_eq!(
            ip_block_seconds(countdown),
            None,
            "a countdown is something to wait out, not an IP block"
        );

        let delay = "<p>Please wait 1 hour, 5 minutes, 30 seconds until the next download</p>";
        assert_eq!(ip_block_seconds(delay), Some(3600 + 300 + 30));
        assert_eq!(
            ip_block_seconds("<p>Please wait until the next download</p>"),
            Some(0),
            "a delay without a parseable duration still has to block the hoster"
        );
        // The XFS base class's own phrasings still apply when FileJoker's does not match.
        assert_eq!(
            ip_block_seconds("<p>You have to wait 2 minutes till next download</p>"),
            Some(120)
        );
        assert_eq!(ip_block_seconds("<p>Here is your file</p>"), None);
    }

    /// The generic XFS countdown markers still apply when FileJoker's own is absent.
    #[test]
    fn the_generic_countdown_markers_are_the_fallback() {
        assert_eq!(
            free_wait_seconds(r#"<span class="seconds">30</span>"#),
            Some(30)
        );
        assert_eq!(free_wait_seconds("<p>no timer here</p>"), None);
    }

    #[test]
    fn detects_the_free_tier_size_limit() {
        assert!(is_free_size_limited(
            "<div class=\"err\">Free user can't download large files</div>"
        ));
        assert!(!is_free_size_limited("<div>Download ready</div>"));
    }

    #[test]
    fn prefers_the_link_matching_the_file_name() {
        let html = r#"<a href="https://filejoker.net/premium">Buy</a>
<a href="https://fs1.filejoker.net/d/abc123/release.rar?x=1">Download File</a>"#;
        assert_eq!(
            direct_link(html, &["release.rar"]).as_deref(),
            Some("https://fs1.filejoker.net/d/abc123/release.rar?x=1")
        );
    }

    #[test]
    fn is_login_wall_delegates_to_the_shared_xfs_common_marker_set() {
        // Calls straight through to `xfs_common::page::is_login_wall` (no hand-copied literals
        // here — see this function's doc comment), so this both proves the delegation and
        // exercises `diagnose`'s own first branch, which is defined in terms of the same function.
        // The login page is the one carrying the `op=login` form; the header's `/login` link
        // is on every guest page and stopped counting with RD-108-28.
        let login_page =
            r#"<form method="POST" name="FL"><input type="hidden" name="op" value="login"></form>"#;
        assert_eq!(
            is_login_wall(login_page),
            xfs_common::page::is_login_wall(login_page)
        );
        assert!(is_login_wall(login_page));
        assert!(diagnose(login_page).contains("login"));
        let guest_page = "<a class=\"nav-link\" href=\"/login\">Login</a>";
        assert_eq!(
            is_login_wall(guest_page),
            xfs_common::page::is_login_wall(guest_page)
        );
        assert!(!is_login_wall(guest_page));
        let logged_in = "<title>Pricing</title><script>var LoginModal = {}</script>";
        assert_eq!(
            is_login_wall(logged_in),
            xfs_common::page::is_login_wall(logged_in)
        );
        assert!(!is_login_wall(logged_in));
        assert!(!diagnose(logged_in).contains("login"));
    }

    #[test]
    fn detects_file_offline_and_premium_only_markers() {
        assert!(is_file_offline("<title>Error</title><b>File Not Found</b>"));
        assert!(!is_file_offline("<title>File</title>"));
        assert!(is_premium_only(
            r#"<div class="premium-download-expand">Premium members only</div>"#
        ));
        assert!(!is_premium_only("<div class=\"other\">text</div>"));
    }

    #[test]
    fn parses_the_wait_seconds_marker() {
        assert_eq!(
            estimated_wait_seconds("<p>Please Wait <b>45</b> seconds before next download</p>"),
            Some(45)
        );
        assert_eq!(estimated_wait_seconds("<p>no wait marker here</p>"), None);
        assert_eq!(
            estimated_wait_seconds("<p>Please Wait <b>abc</b> seconds</p>"),
            None
        );
    }

    /// Regression for the review finding that a bare `"Wait "` marker could match a stray,
    /// unrelated occurrence of the word elsewhere on the page (e.g. unrelated copy mentioning
    /// waiting) ahead of the real `"Please Wait ..."` countdown.
    #[test]
    fn stray_wait_text_without_the_please_prefix_does_not_cause_a_false_match() {
        assert_eq!(
            estimated_wait_seconds(
                "<p>Please wait while we verify your session.</p><p>Please Wait <b>45</b> seconds</p>"
            ),
            Some(45),
            "an unrelated, lowercase \"wait\" (not the marker's exact \"Please Wait \" casing) must not pre-empt the real countdown"
        );
        assert_eq!(
            estimated_wait_seconds("<p>Wait <b>99</b> seconds</p>"),
            None,
            "a bare \"Wait \" marker without the \"Please \" prefix must not match"
        );
    }
}

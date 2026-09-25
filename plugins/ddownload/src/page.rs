//! Thin wrapper binding `xfs-common`'s generalized XFS page helpers to ddownload's own
//! parameterization (op marker, premium button label, direct-link domain). Extracted to
//! `xfs-common` in Task 11; every function here delegates unchanged so `native`/`guest` call
//! sites and this module's own tests (kept identical, now exercising the wrapper) need no
//! changes — this is the "byte-for-byte" proof that the refactor preserved ddownload's behavior.
//!
//! The account-less (free) flow's wrappers were added on top of that, mirroring
//! `plugins/katfile/src/page.rs`. IMPL-VERIFY against JD's `DdownloadCom.java` (rev fetched
//! 2026-09-02 from `mycodedoesnotcompile2/jdownloader_mirror`,
//! `svn_trunk/src/jd/plugins/hoster/DdownloadCom.java`) and its `XFileSharingProBasic` base class
//! (`svn_trunk/src/org/jdownloader/plugins/components/XFileSharingProBasic.java`):
//! - **`op` values / free button label**: `DdownloadCom` overrides neither
//!   `findFormDownload1Free` nor `findFormDownload2Free`; its `doFree` only shows the "free
//!   dialog" and then calls `super.doFree`. So the base class's own values apply verbatim —
//!   `XFileSharingProBasic.findFormDownload1Free` looks up the form by input field `op` =
//!   `download1`, removes `method_premium`, and fills `method_free` with the page's own value or,
//!   when it is missing/empty/disabled, the literal `"Free Download"`; the second form is found by
//!   `op` = `download2` (`findFormDownload2Free`'s `getFormByInputFieldKeyValue("op",
//!   "download2")` fallback). Those are exactly [`OP_DOWNLOAD1`]/[`OP_DOWNLOAD2`]/[`FREE_BUTTON`]
//!   below and match KatFile's defaults.
//! - **Countdown**: `DdownloadCom.regexWaittime` (dated 2026-04-21 in the source) overrides the
//!   base class with `id="dk2CountdownNum"[^>]*>\s*(\d{1,2})` and falls back to `super`, which is
//!   what [`free_wait_seconds`] reproduces ([`dk2_countdown_seconds`] first, then
//!   `xfs_common::free::countdown_seconds`).
//! - **`adblock_detected`**: both `XFileSharingProBasic.findFormDownload2Free` and
//!   `DdownloadCom.handleCaptcha` set this field to `"0"` when the form carries it (JD's comment:
//!   "This might increase downloadspeed for free users"), reproduced by
//!   [`with_adblock_cleared`].
//! - **Measured 2026-09-17** (RD-108-28, `tests/fixtures/file-page-2026-09-17.html`): the file
//!   page no longer carries a `download1` form at all. It carries the `download2` form directly
//!   (`op`, `id`, `rand`, `referer`, `method_free`, `method_premium`), a Cloudflare Turnstile
//!   widget inside that form, and ddownload's `dk2CountdownNum` countdown. The flow therefore
//!   skips the first step when the page already offers the second, and the widget scan is
//!   scoped to the form: the page's stylesheet mentions `h-captcha` long before the widget, and
//!   a page-wide scan took that for the captcha kind. `DdownloadCom.handleCaptcha` expects any
//!   of reCAPTCHA v2, hCaptcha and Turnstile, and `xfs_common::free::widget_marker` still
//!   recognises all three.

const OP_DOWNLOAD1: &str = "download1";
const OP_DOWNLOAD2: &str = "download2";
const PREMIUM_BUTTON: &str = "Premium Download";
/// `XFileSharingProBasic.findFormDownload1Free`'s own fallback label — see the module doc.
const FREE_BUTTON: &str = "Free Download";
/// Hosts a download link may point at: the site itself plus the CDN it delivers from,
/// matching `manifest.toml`'s `download_domains`. The answer page of a free download often
/// links straight to the CDN, so restricting the scan to `ddownload.com` would report "no
/// link" for a flow that actually succeeded.
const DOWNLOAD_DOMAINS: &[&str] = &["ddownload.com", "*.zeuscdn.org"];

/// The `op=login` form of the login page.
#[must_use]
pub(crate) fn login_form(html: &str) -> Option<xfs_common::login::LoginForm> {
    xfs_common::login::login_form(html)
}

/// The sign-in submission, with the host's credential markers in place of the credentials.
#[must_use]
pub(crate) fn login_body(form: &xfs_common::login::LoginForm) -> Vec<u8> {
    xfs_common::login::login_body(
        form,
        "{{username}}",
        &format!(
            "{{{{secret:{}}}}}",
            crate::resolver::api::PASSWORD_REFERENCE
        ),
    )
}

/// What the site made of a sign-in attempt.
#[must_use]
/// The captcha widget guarding the login form, if the site is showing one.
pub(crate) fn login_challenge(html: &str) -> Option<xfs_common::free::WidgetMarker> {
    xfs_common::login::login_challenge(html)
}

/// The login form with the solved captcha token added.
pub(crate) fn with_challenge_token(
    form: &xfs_common::login::LoginForm,
    kind: xfs_common::free::WidgetKind,
    token: &str,
) -> xfs_common::login::LoginForm {
    xfs_common::login::with_challenge_token(form, kind, token)
}

pub(crate) fn login_outcome(set_cookies: &[String], html: &str) -> xfs_common::login::LoginOutcome {
    xfs_common::login::login_outcome(set_cookies, html)
}

/// The account's API key as the signed-in account page renders it.
#[must_use]
pub(crate) fn api_key(html: &str) -> Option<String> {
    xfs_common::login::api_key(html)
}

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

/// Turns raw form fields into the free submission (keeps `method_free`, drops the premium
/// marker) — the counterpart of [`premium_form`].
#[must_use]
pub(crate) fn free_form(fields: &[(String, String)]) -> Vec<(String, String)> {
    xfs_common::free::free_form(fields, FREE_BUTTON)
}

/// Clears ddownload's `adblock_detected` field when the form carries it, the way JD's
/// `DdownloadCom.handleCaptcha` (and the base class's `findFormDownload2Free`) do — see the
/// module doc. Forms without the field are returned unchanged; the field is never added.
#[must_use]
pub(crate) fn with_adblock_cleared(fields: &[(String, String)]) -> Vec<(String, String)> {
    fields
        .iter()
        .map(|(name, value)| {
            if name == "adblock_detected" {
                (name.clone(), "0".to_owned())
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect()
}

pub(crate) use xfs_common::page::SessionVerdict;

/// What the page says about the visitor's session, delegated unchanged.
///
/// The three-state answer, not the bare `shows_signed_in` predicate this wrapper used to
/// forward: the predicate says whether the sign-out link is there, which is not the same
/// question as whether the page is evidence about the account at all. Collapsing the two was
/// the RD-108-28 review's finding — a page carrying neither marker is unrecognized, not a
/// broken account.
#[must_use]
pub(crate) fn session_verdict(html: &str) -> SessionVerdict {
    xfs_common::page::session_verdict(html)
}

/// The captcha widget guarding the download form, with the site key needed to solve it.
///
/// Scoped to the `download2` form when the page carries one, the way JD's `handleCaptcha` is
/// handed the form rather than the page: on the file page measured on 2026-09-17 the
/// stylesheet mentions `h-captcha` two thousand lines before the Turnstile widget, and a
/// page-wide scan reported an hCaptcha challenge carrying the Turnstile's site key. When the
/// form carries no widget — or there is no such form, as in the answer to `download1` on an
/// older installation — the page is scanned whole, so a widget the template put next to the
/// form rather than inside it is still answered instead of the form being posted bare. That
/// page-wide scan runs on the page without its `<style>` and `<script>` blocks, because the
/// decoy that made the scoping necessary lives in one of them and would otherwise win again
/// the moment the form is empty.
#[must_use]
pub(crate) fn widget_marker(html: &str) -> Option<xfs_common::free::WidgetMarker> {
    xfs_common::page::form_html(html, OP_DOWNLOAD2)
        .and_then(widget_marker_outside_markup)
        .or_else(|| widget_marker_outside_markup(html))
}

/// The widget scan on the text without its `<style>` and `<script>` blocks — the places a
/// widget's class name appears without a widget. Shared with the session check in
/// `xfs_common::page`, which has the same decoy to avoid.
fn widget_marker_outside_markup(html: &str) -> Option<xfs_common::free::WidgetMarker> {
    xfs_common::free::widget_marker(&xfs_common::page::without_style_script_and_comments(html))
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

/// Seconds to wait before the free download may be requested: ddownload's own
/// `id="dk2CountdownNum"` marker first, then the XFS base class's generic countdown markers —
/// exactly the order `DdownloadCom.regexWaittime` uses (see the module doc).
#[must_use]
pub(crate) fn free_wait_seconds(html: &str) -> Option<u64> {
    dk2_countdown_seconds(html).or_else(|| xfs_common::free::countdown_seconds(html))
}

/// Seconds this IP must wait for another free download, or `Some(0)` when the page states a
/// limit without naming a duration. `DdownloadCom.checkErrors` adds only HTTP 429/500 handling
/// (already covered by `api::ensure_http_status`) on top of `super.checkErrors`, so the base
/// class's own phrasings — `xfs_common::free::ip_block_seconds` — apply unchanged.
#[must_use]
pub(crate) fn ip_block_seconds(html: &str) -> Option<u64> {
    xfs_common::free::ip_block_seconds(html)
}

/// Whether the page says the captcha answer was rejected.
#[must_use]
pub(crate) fn is_wrong_captcha(html: &str) -> bool {
    xfs_common::free::is_wrong_captcha(html)
}

/// ddownload's own countdown marker, `id="dk2CountdownNum"` (JD `DdownloadCom.regexWaittime`,
/// dated 2026-04-21). Deliberately reads the full run of digits rather than JD's `\d{1,2}`, so a
/// three-digit countdown is waited out in full instead of being truncated. `None` if the marker
/// is absent, carries no digits, or names zero seconds.
#[must_use]
fn dk2_countdown_seconds(html: &str) -> Option<u64> {
    const MARKER: &str = "id=\"dk2CountdownNum\"";
    let after_marker = &html[html.find(MARKER)? + MARKER.len()..];
    let after_tag = &after_marker[after_marker.find('>')? + 1..];
    let digits: String = after_tag
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let seconds: u64 = digits.parse().ok()?;
    (seconds > 0).then_some(seconds)
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

/// Encodes form fields as `application/x-www-form-urlencoded`.
#[must_use]
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    xfs_common::page::encode_form(fields)
}

/// Finds the premium direct link on the page returned after submitting the form.
#[must_use]
pub(crate) fn direct_link(html: &str, hints: &[&str]) -> Option<String> {
    xfs_common::page::direct_link_any(html, hints, DOWNLOAD_DOMAINS)
}

#[cfg(test)]
mod tests {
    use super::{
        diagnose, direct_link, download_form, download1_form, encode_form, free_form,
        free_wait_seconds, ip_block_seconds, premium_form, widget_marker, with_adblock_cleared,
    };

    const PAGE: &str = r#"<html><body>
<form name="F1" method="POST" action="" style="display:contents;">
  <input type="hidden" name="op" value="download2">
  <input type="hidden" name="id" value="z31889n8peey">
  <input type="hidden" name="rand" value="2wde33y7krah">
  <input type="hidden" name="referer" value="">
  <input type="hidden" name="method_free" value="">
  <input type="hidden" name="method_premium" value="">
  <input type="email" class="rm-input" id="rm-email" name="email">
</form></body></html>"#;

    #[test]
    fn extracts_hidden_fields_of_the_download_form() {
        let fields = download_form(PAGE).expect("form");
        assert_eq!(
            fields,
            [
                ("op", "download2"),
                ("id", "z31889n8peey"),
                ("rand", "2wde33y7krah"),
                ("referer", ""),
                ("method_free", ""),
                ("method_premium", ""),
            ]
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
        );
        assert_eq!(
            encode_form(&fields),
            b"op=download2&id=z31889n8peey&rand=2wde33y7krah&referer=&method_free=&method_premium="
        );
        assert!(download_form("<html><form><input name='x'></form></html>").is_none());
    }

    #[test]
    fn prefers_the_link_matching_the_file_name() {
        let html = r#"<a href="https://ddownload.com/premium">Buy</a>
<a href="https://fs12.ddownload.com/d/abc123/release.rar?x=1&amp;y=2" id="direct">Download</a>"#;
        assert_eq!(
            direct_link(html, &["release.rar"]).as_deref(),
            Some("https://fs12.ddownload.com/d/abc123/release.rar?x=1&y=2")
        );
        assert_eq!(
            direct_link(html, &["other.rar"]).as_deref(),
            Some("https://fs12.ddownload.com/d/abc123/release.rar?x=1&y=2")
        );
        assert!(direct_link("<a href=\"https://example.com/d/x\">", &["x"]).is_none());
    }

    /// DDownload delivers from `zeuscdn.org`, which `manifest.toml` already allows as a
    /// download domain. The free flow's answer page links straight there, so the link scan
    /// has to accept it (including the port those hosts use) or the download reports "no
    /// link" despite having succeeded.
    #[test]
    fn the_delivery_cdn_is_accepted_as_a_download_domain() {
        let html = r#"<a href="https://ddownload.com/premium">Buy</a>
<a href="https://eu-hydra5.zeuscdn.org:183/d/tok3n/release.rar">Download</a>"#;

        assert_eq!(
            direct_link(html, &["release.rar"]).as_deref(),
            Some("https://eu-hydra5.zeuscdn.org:183/d/tok3n/release.rar")
        );
        assert!(
            direct_link(
                "<a href=\"https://evil.test/d/x/release.rar\">x</a>",
                &["release.rar"]
            )
            .is_none(),
            "an unrelated host must never be taken as a download link"
        );
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
        let without = vec![("op".to_owned(), "download2".to_owned())];
        assert_eq!(premium_form(&without).len(), 2);
    }

    const DOWNLOAD1_PAGE: &str = r#"<html><body>
<form name="F1" method="POST" action="">
  <input type="hidden" name="op" value="download1">
  <input type="hidden" name="id" value="z31889n8peey">
  <input type="hidden" name="method_free" value="Free Download">
  <input type="hidden" name="method_premium" value="">
  <input type="hidden" name="adblock_detected" value="1">
</form></body></html>"#;

    /// The first free step: the `download1` form is found by its own `op` marker, and the free
    /// submission keeps `method_free` while dropping `method_premium` (JD
    /// `XFileSharingProBasic.findFormDownload1Free`).
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
    }

    /// JD's `DdownloadCom.handleCaptcha` clears this field rather than removing or adding it.
    #[test]
    fn adblock_detected_is_cleared_only_when_the_form_carries_it() {
        let fields = download1_form(DOWNLOAD1_PAGE).expect("download1 form");
        let cleared = with_adblock_cleared(&fields);
        assert_eq!(
            cleared.iter().find(|(name, _)| name == "adblock_detected"),
            Some(&("adblock_detected".to_owned(), "0".to_owned()))
        );
        let without = vec![("op".to_owned(), "download2".to_owned())];
        assert_eq!(with_adblock_cleared(&without), without);
    }

    /// ddownload's own `dk2CountdownNum` marker wins over the base class's generic ones, and the
    /// generic ones still apply when it is absent (JD `DdownloadCom.regexWaittime`).
    #[test]
    fn the_countdown_prefers_ddownloads_own_marker() {
        assert_eq!(
            free_wait_seconds(
                r#"<span id="dk2CountdownNum" class="c">17</span><span class="seconds">99</span>"#
            ),
            Some(17)
        );
        assert_eq!(
            free_wait_seconds(r#"<span class="seconds">30</span>"#),
            Some(30)
        );
        assert_eq!(free_wait_seconds("<p>no timer here</p>"), None);
        assert_eq!(
            free_wait_seconds(r#"<span id="dk2CountdownNum">0</span>"#),
            None,
            "a countdown that already reads zero is nothing to wait for"
        );
    }

    #[test]
    fn a_free_download_limit_is_recognised_with_and_without_a_duration() {
        assert_eq!(
            ip_block_seconds("<p>You have to wait 45 minutes, 30 seconds till next download</p>"),
            Some(45 * 60 + 30)
        );
        assert_eq!(
            ip_block_seconds("<p>You have reached the download-limit</p>"),
            Some(0)
        );
        assert_eq!(ip_block_seconds("<p>Here is your file</p>"), None);
    }

    #[test]
    fn diagnose_explains_login_pages_errors_and_titles() {
        assert!(
            diagnose("<form><input type=\"hidden\" name=\"op\" value=\"login\"></form>")
                .contains("login")
        );
        // The header's login link is on every page a guest sees (four times on the file page
        // measured on 2026-09-17); it must not be read as a login wall.
        assert!(
            !diagnose(
                "<title>Download release.rar</title><a class=\"nav-link\" href=\"/login\">Login</a>"
            )
            .contains("login")
        );
        // Every ddownload page ships the login modal script, even for logged-in users.
        assert!(
            !diagnose("<title>Pricing - DDownload</title><script>var LoginModal = {}</script>")
                .contains("login")
        );
        assert_eq!(
            diagnose("<div class=\"err\"><b>File</b> not  found</div>"),
            "page message: File not found"
        );
        assert_eq!(
            diagnose("<title>Please wait</title>"),
            "page \"Please wait\" contains no premium link"
        );
    }

    /// The measured page's stylesheet names `h-captcha` long before the form's Turnstile
    /// widget; the scan has to start at the form, or it reports the wrong challenge.
    #[test]
    fn the_widget_scan_starts_at_the_download_form() {
        let page = r#"<style>div[class*="h-captcha"] { margin: 0 }</style>
<form name="F1" method="POST" action="">
  <input type="hidden" name="op" value="download2">
  <div class="cf-turnstile" data-sitekey="0x4AAAAAABm53D0OJNkESa1O" id="cf-turnstile-widget"></div>
</form>"#;
        let marker = widget_marker(page).expect("a challenge");
        assert_eq!(marker.kind, xfs_common::free::WidgetKind::Turnstile);
        assert_eq!(marker.site_key, "0x4AAAAAABm53D0OJNkESa1O");
        // Without the form the page is scanned whole, as before.
        let bare = r#"<div class="g-recaptcha" data-sitekey="6Lc-free-key"></div>"#;
        assert_eq!(
            widget_marker(bare).map(|marker| marker.kind),
            Some(xfs_common::free::WidgetKind::RecaptchaV2)
        );
        // A widget beside the form rather than inside it is still found; a form without one
        // must not be posted bare just because the scoped scan came up empty — and the
        // page-wide scan that finds it must not fall for the stylesheet's decoy either, or the
        // empty form would re-open the very bug the scoping closed.
        let beside = r#"<style>div[class*="h-captcha"] { margin: 0 }</style>
<script>if (typeof grecaptcha !== 'undefined') { /* g-recaptcha loader */ }</script>
<form name="F1"><input type="hidden" name="op" value="download2"></form>
<div class="cf-turnstile" data-sitekey="0x4AAAAAABm53D0OJNkESa1O"></div>"#;
        let marker = widget_marker(beside).expect("the widget beside the form");
        assert_eq!(marker.kind, xfs_common::free::WidgetKind::Turnstile);
        assert_eq!(marker.site_key, "0x4AAAAAABm53D0OJNkESa1O");
        // The decoy's tags in upper case must not slip through either.
        let shouting = beside
            .replace("<style>", "<STYLE>")
            .replace("</style>", "</STYLE>");
        assert_eq!(
            widget_marker(&shouting).map(|marker| marker.kind),
            Some(xfs_common::free::WidgetKind::Turnstile)
        );
    }
}

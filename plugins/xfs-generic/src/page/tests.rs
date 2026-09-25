//! The page rules against XFS-shaped excerpts.
//!
//! These assert the binding rather than the parsing: `xfs-common` has its own tests for the
//! parsers themselves, and what can break here is a wrong `op` value or a wrong button label —
//! a plugin that looks for `op=download3` finds nothing and reports an unrecognised page on a
//! site that works perfectly.

use super::*;

const FILE_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download1">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="method_free" value="Slow Download">
<input type="hidden" name="method_premium" value="Premium Download">
</form>"#;

const SECOND_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
</form>
<span id="countdown_str">Wait <span id="xyz">17</span> seconds</span>"#;

fn value<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn the_first_step_is_read_from_the_download1_form() {
    let fields = download1_form(FILE_PAGE).expect("the download1 form");
    assert_eq!(value(&fields, "op"), Some("download1"));
    assert_eq!(value(&fields, "id"), Some("abc123xyz"));
    assert!(download2_form(FILE_PAGE).is_none());
}

#[test]
fn the_second_step_is_read_from_the_download2_form() {
    let fields = download2_form(SECOND_PAGE).expect("the download2 form");
    assert_eq!(value(&fields, "op"), Some("download2"));
    assert_eq!(value(&fields, "rand"), Some("r4nd"));
}

#[test]
fn the_free_submission_keeps_the_page_s_own_button_label() {
    // The label varies between clones ("Slow Download" here), so the page's value is echoed
    // back rather than replaced with this plugin's fallback.
    let fields = free_form(&download1_form(FILE_PAGE).expect("form"));
    assert_eq!(value(&fields, "method_free"), Some("Slow Download"));
    assert_eq!(value(&fields, "method_premium"), None);
}

#[test]
fn the_free_submission_falls_back_to_the_script_s_own_label() {
    for page in [
        r#"<form><input type="hidden" name="op" value="download1"></form>"#,
        r#"<form><input type="hidden" name="op" value="download1">
<input type="hidden" name="method_free" value=""></form>"#,
    ] {
        let fields = free_form(&download1_form(page).expect("form"));
        assert_eq!(
            value(&fields, "method_free"),
            Some("Free Download"),
            "{page}"
        );
    }
}

#[test]
fn the_countdown_is_read_from_the_second_page() {
    assert_eq!(free_wait_seconds(SECOND_PAGE), Some(17));
    assert_eq!(free_wait_seconds(FILE_PAGE), None);
}

#[test]
fn a_captcha_widget_is_recognised_with_its_site_key() {
    let page = r#"<div class="g-recaptcha" data-sitekey="6Lc-site-key"></div>"#;
    let marker = widget_marker(page).expect("a widget");
    assert_eq!(marker.site_key, "6Lc-site-key");
    assert_eq!(marker.kind.response_field(), "g-recaptcha-response");
}

#[test]
fn a_direct_link_is_taken_only_from_the_site_itself() {
    let page = r#"<a href="https://dl7.clone.test/d/abc123xyz/release.rar">Download</a>"#;
    assert_eq!(
        direct_link(page, &["release.rar"], &["clone.test"]).as_deref(),
        Some("https://dl7.clone.test/d/abc123xyz/release.rar")
    );
    assert_eq!(direct_link(page, &["release.rar"], &["other.test"]), None);
}

#[test]
fn a_rejected_captcha_is_recognised() {
    assert!(is_wrong_captcha("<div>Wrong captcha</div>"));
    assert!(!is_wrong_captcha(SECOND_PAGE));
}

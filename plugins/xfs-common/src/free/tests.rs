//! What the free flow's page helpers read, checked without a host.

use plugin_common::failure::HttpError;
use plugin_common::{CaptchaChallenge, FailureKind};

use super::{
    FreeWords, WidgetKind, challenge_for, countdown_seconds, free_form, ip_block_seconds,
    is_wrong_captcha, unit_sum_seconds, widget_marker, with_captcha_token,
};

fn fields() -> Vec<(String, String)> {
    [
        ("op", "download1"),
        ("id", "abc123"),
        ("method_free", ""),
        ("method_premium", ""),
    ]
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .to_vec()
}

/// The exact inverse of `page::premium_form`: keeping `method_free` is what makes an
/// account-less download possible at all.
#[test]
fn free_form_keeps_the_free_marker_and_drops_the_premium_one() {
    let free = free_form(&fields(), "Free Download");
    assert!(free.iter().all(|(name, _)| name != "method_premium"));
    assert_eq!(
        free.iter().find(|(name, _)| name == "method_free"),
        Some(&("method_free".to_owned(), "Free Download".to_owned()))
    );

    let without = vec![("op".to_owned(), "download1".to_owned())];
    assert_eq!(free_form(&without, "Free Download").len(), 2);
}

#[test]
fn a_captcha_token_replaces_any_existing_response_field() {
    let base = vec![("g-recaptcha-response".to_owned(), "stale".to_owned())];
    let submitted = with_captcha_token(&base, WidgetKind::RecaptchaV2, "fresh");
    assert_eq!(
        submitted,
        [("g-recaptcha-response".to_owned(), "fresh".to_owned())]
    );
    assert_eq!(
        with_captcha_token(&[], WidgetKind::Turnstile, "t")[0].0,
        "cf-turnstile-response"
    );
    assert_eq!(WidgetKind::HCaptcha.response_field(), "h-captcha-response");
}

#[test]
fn each_widget_is_recognised_with_its_site_key() {
    let recaptcha = r#"<div class="g-recaptcha" data-sitekey="6Lc-abc"></div>"#;
    assert_eq!(
        widget_marker(recaptcha),
        Some(super::WidgetMarker {
            kind: WidgetKind::RecaptchaV2,
            site_key: "6Lc-abc".to_owned()
        })
    );

    let turnstile = r#"<div data-sitekey="0x4AAA" class="cf-turnstile"></div>"#;
    let marker = widget_marker(turnstile).expect("turnstile");
    assert_eq!(marker.kind, WidgetKind::Turnstile);
    assert_eq!(marker.site_key, "0x4AAA");

    let hcaptcha = r#"<div class="h-captcha" data-sitekey='hk-1'></div>"#;
    assert_eq!(widget_marker(hcaptcha).expect("hcaptcha").site_key, "hk-1");

    assert_eq!(widget_marker("<form></form>"), None);
    assert_eq!(
        widget_marker(r#"<div class="g-recaptcha"></div>"#),
        None,
        "a marker without a usable site key cannot be solved"
    );
}

#[test]
fn the_countdown_is_read_from_the_usual_markers() {
    assert_eq!(
        countdown_seconds(r#"<span id="countdown_str">Wait <span id="xyz">45</span></span>"#),
        Some(45)
    );
    assert_eq!(
        countdown_seconds(r#"<span class="seconds">30</span>"#),
        Some(30)
    );
    assert_eq!(countdown_seconds("<p>no timer here</p>"), None);
    // A far-away digit must not be mistaken for the timer.
    let distant = format!(r#"<span class="seconds"></span>{}17"#, " ".repeat(500));
    assert_eq!(countdown_seconds(&distant), None);
}

#[test]
fn a_stated_wait_is_summed_across_its_units() {
    assert_eq!(
        ip_block_seconds("<p>You have to wait 2 hours, 30 minutes, 15 seconds</p>"),
        Some(2 * 3600 + 30 * 60 + 15)
    );
    assert_eq!(
        ip_block_seconds("You have to wait 45 minutes till next download"),
        Some(45 * 60)
    );
}

/// A limit without a duration must still register, so the hoster is held back with the
/// scheduler's own default instead of being retried at once.
#[test]
fn a_limit_without_a_duration_still_reports_a_block() {
    assert_eq!(
        ip_block_seconds("<p>You have reached the download-limit</p>"),
        Some(0)
    );
    assert_eq!(
        ip_block_seconds("<p>you can download only one file at a time</p>"),
        Some(0)
    );
    assert_eq!(ip_block_seconds("<p>Here is your file</p>"), None);
}

/// The unit summing [`ip_block_seconds`] uses, exposed for plugins whose site words the
/// surrounding sentence differently (FileJoker's "Please wait ... until the next download").
#[test]
fn unit_sum_seconds_adds_hours_minutes_and_seconds_and_stops_at_an_unknown_unit() {
    assert_eq!(
        unit_sum_seconds(" 1 hour, 2 minutes, 3 seconds until the next download"),
        Some(3600 + 120 + 3)
    );
    assert_eq!(unit_sum_seconds(" 90 seconds"), Some(90));
    assert_eq!(unit_sum_seconds(" no numbers at all"), None);
    assert_eq!(
        unit_sum_seconds(" 5 minutes and 3 files"),
        Some(300),
        "a number whose unit is not a duration ends the run"
    );
}

#[test]
fn a_rejected_captcha_is_recognised() {
    assert!(is_wrong_captcha("<div class=\"err\">Wrong captcha</div>"));
    assert!(is_wrong_captcha("the verification code is incorrect"));
    assert!(!is_wrong_captcha("<div>Download ready</div>"));
}

fn http_error(status: u16) -> String {
    format!("status {status}")
}

fn no_form(diagnosis: &str) -> String {
    format!("no form: {diagnosis}")
}

fn no_link(diagnosis: &str) -> String {
    format!("no link: {diagnosis}")
}

fn limit(seconds: Option<u64>) -> String {
    format!("limit {seconds:?}")
}

const WORDS: FreeWords = FreeWords {
    http_error: HttpError {
        code: "x.http_error",
        text: http_error,
    },
    no_free_form: ("x.no_free_form", no_form),
    no_free_link: ("x.no_free_link", no_link),
    free_limit_reached: ("x.free_limit_reached", limit),
};

/// The free flow's dead ends carry the plugin's codes and the page's own diagnosis.
#[test]
fn the_free_flows_failures_are_named_in_the_plugins_words() {
    let page = "<title>File Not Found</title>";
    let form = WORDS.no_free_form(page);
    assert_eq!(form.kind, FailureKind::Permanent);
    assert_eq!(form.code.as_deref(), Some("x.no_free_form"));
    assert_eq!(form.params[0].0, "diagnosis");
    assert_eq!(form.message, no_form(&form.params[0].1));
    let link = WORDS.no_free_link(page);
    assert_eq!(link.code.as_deref(), Some("x.no_free_link"));
    assert_eq!(link.params, form.params);

    assert!(WORDS.free_page_failure("<p>Here is your file</p>").is_ok());
    let blocked = WORDS
        .free_page_failure("<p>You have to wait 2 minutes, 5 seconds</p>")
        .expect_err("a limit");
    assert_eq!(blocked.kind, FailureKind::IpBlocked(Some(125)));
    assert_eq!(blocked.code.as_deref(), Some("x.free_limit_reached"));
    assert_eq!(
        blocked.params,
        vec![("wait_seconds".to_owned(), "125".to_owned())]
    );
}

#[test]
fn each_widget_becomes_its_challenge() {
    let marker = widget_marker(r#"<div class="cf-turnstile" data-sitekey="0x4AAA"></div>"#)
        .expect("turnstile");
    match challenge_for(&marker, "https://xfs.invalid/abc123") {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, "0x4AAA");
            assert_eq!(widget.page_url, "https://xfs.invalid/abc123");
            assert!(!widget.invisible);
        }
        other => panic!("expected a Turnstile challenge, got {other:?}"),
    }
}

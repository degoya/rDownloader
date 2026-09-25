//! Unit coverage for the free-flow page parsers. Every case here runs without a host, which is
//! why the parsing lives in [`crate::page`] rather than in either adapter.

use super::*;

const FILE_PAGE: &str = r#"<html><head><title>Download release.rar</title>
<script src="https://www.google.com/recaptcha/api.js" async defer></script></head>
<body>
<script>
  var fid = 7654321;
  var secs = 30;
  var startTimerUrl = '/download/AjaxStartTimer';
</script>
<div class="g-recaptcha" data-sitekey="6Lc-free-key"></div>
</body></html>"#;

#[test]
fn timer_markers_reads_all_three_javascript_variables() {
    let markers = timer_markers(FILE_PAGE).expect("markers");
    assert_eq!(markers.start_timer_url, "/download/AjaxStartTimer");
    assert_eq!(markers.fid, 7_654_321);
    assert_eq!(markers.wait_seconds, 30);
}

#[test]
fn timer_markers_reports_none_when_any_marker_is_missing() {
    assert!(timer_markers("<html><body>nothing here</body></html>").is_none());
    assert!(timer_markers("<script>var fid = 1; var secs = 5;</script>").is_none());
    assert!(
        timer_markers("<script>var startTimerUrl = '/x'; var secs = 5;</script>").is_none(),
        "a missing fid must not be guessed"
    );
}

/// A longer identifier that merely contains the variable name must not be mistaken for it.
#[test]
fn timer_markers_ignores_similarly_named_identifiers() {
    let html = r#"<script>var fidelity = 'high'; var fid = 42; var secsElapsed = 900;
      var secs = 7; var startTimerUrlBase = 'x'; var startTimerUrl = "/t";</script>"#;
    let markers = timer_markers(html).expect("markers");
    assert_eq!(markers.fid, 42);
    assert_eq!(markers.wait_seconds, 7);
    assert_eq!(markers.start_timer_url, "/t");
}

#[test]
fn recaptcha_site_key_prefers_the_widget_attribute() {
    assert_eq!(
        recaptcha_site_key(FILE_PAGE).as_deref(),
        Some("6Lc-free-key")
    );
}

#[test]
fn recaptcha_site_key_falls_back_to_a_key_carried_in_the_script_url() {
    let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=6LcSUAsUAAAAAKBeQQE893pf0Io66"></script>"#;
    assert_eq!(
        recaptcha_site_key(html).as_deref(),
        Some("6LcSUAsUAAAAAKBeQQE893pf0Io66")
    );
}

/// `render=explicit` is a rendering mode, not a site key.
#[test]
fn recaptcha_site_key_ignores_the_explicit_render_mode() {
    let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=explicit"></script>"#;
    assert_eq!(recaptcha_site_key(html), None);
}

#[test]
fn ip_block_seconds_parses_the_delay_between_downloads_notice() {
    let html =
        "<div>Delay between downloads must be not less than 120 min. Don`t want to wait?</div>";
    assert_eq!(ip_block_seconds(html), Some(120 * 60));
}

#[test]
fn ip_block_seconds_reports_stated_limits_without_a_duration() {
    for html in [
        ">You have reached your daily downloads limit",
        ">You have reached your hourly downloads limit.",
        ">Error. Link expired. You have reached your daily limit of downloads.",
        "You can`t download more than 1 file at a time in free mode.",
        "You can`t download not more than 1 file at a time",
        "<div>Wish to remove the restrictions?</div>",
    ] {
        assert_eq!(ip_block_seconds(html), Some(0), "{html}");
    }
}

#[test]
fn ip_block_seconds_reports_the_already_downloading_notice_with_its_own_wait() {
    assert_eq!(
        ip_block_seconds("<div>File is already downloading</div>"),
        Some(60)
    );
}

#[test]
fn ip_block_seconds_reports_none_for_an_ordinary_page() {
    assert_eq!(ip_block_seconds(FILE_PAGE), None);
}

#[test]
fn is_wrong_captcha_matches_both_rejection_phrases() {
    assert!(is_wrong_captcha(
        "<div>Please fix the following input errors</div>"
    ));
    assert!(is_wrong_captcha(
        "<span>The verification code is incorrect</span>"
    ));
    assert!(!is_wrong_captcha(FILE_PAGE));
}

const CAPTCHA_PAGE: &str = r#"<html><body>
<form id="captchaform" method="post" action="/download/captcha?x=1&amp;y=2">
<input type="hidden" name="DownloadCaptchaForm[captchaType]" value="recaptcha">
<input type="hidden" name="DownloadCaptchaForm[verifyCode]" value="">
<input type="text" name="unrelated">
<div class="g-recaptcha" data-sitekey="6Lc-captcha-key"></div>
</form></body></html>"#;

#[test]
fn captcha_form_reads_the_action_and_the_hidden_fields() {
    let form = captcha_form(CAPTCHA_PAGE).expect("form");
    assert_eq!(form.action.as_deref(), Some("/download/captcha?x=1&y=2"));
    assert_eq!(
        form.fields,
        vec![
            (
                "DownloadCaptchaForm[captchaType]".to_owned(),
                "recaptcha".to_owned()
            ),
            ("DownloadCaptchaForm[verifyCode]".to_owned(), String::new()),
            ("unrelated".to_owned(), String::new()),
        ]
    );
}

/// The id must belong to the `<form>` tag itself, not to something nested inside another form.
#[test]
fn captcha_form_ignores_an_id_on_a_child_element() {
    let html = r#"<form method="post"><div id="captchaform"></div></form>"#;
    assert!(captcha_form(html).is_none());
}

#[test]
fn captcha_form_reports_none_when_no_captcha_is_asked_for() {
    assert!(captcha_form(FILE_PAGE).is_none());
}

#[test]
fn with_captcha_token_fills_both_token_fields_exactly_once() {
    let form = captcha_form(CAPTCHA_PAGE).expect("form");
    let submitted = with_captcha_token(&form.fields, "tok3n");
    let verify: Vec<&(String, String)> = submitted
        .iter()
        .filter(|(name, _)| name == "DownloadCaptchaForm[verifyCode]")
        .collect();
    assert_eq!(verify.len(), 1);
    assert_eq!(verify[0].1, "tok3n");
    assert!(
        submitted
            .iter()
            .any(|(name, value)| name == "g-recaptcha-response" && value == "tok3n")
    );
    // The form's other fields survive.
    assert!(
        submitted
            .iter()
            .any(|(name, _)| name == "DownloadCaptchaForm[captchaType]")
    );
}

#[test]
fn final_download_link_reads_the_session_url() {
    let html = r#"<script>window.open('https://pr_srv.rapidgator.net//?r=download/index&amp;session_id=Ab12Cd34');</script>"#;
    assert_eq!(
        final_download_link(html).as_deref(),
        Some("https://pr_srv.rapidgator.net//?r=download/index&session_id=Ab12Cd34")
    );
}

#[test]
fn final_download_link_falls_back_to_location_href() {
    let html =
        r#"<script>location.href = 'https://pr7.rapidgator.net/d/tok3n/release.rar';</script>"#;
    assert_eq!(
        final_download_link(html).as_deref(),
        Some("https://pr7.rapidgator.net/d/tok3n/release.rar")
    );
}

/// JD's third, much looser fallback is deliberately unimplemented (see the module doc): the
/// captcha page's own URL must never be handed back as a download link.
#[test]
fn final_download_link_does_not_match_the_captcha_page_itself() {
    assert_eq!(
        final_download_link(r#"<a href="https://rapidgator.net/download/captcha">retry</a>"#),
        None
    );
}

#[test]
fn timer_state_accepts_a_string_and_a_numeric_sid() {
    let started = timer_state(br#"{"state":"started","sid":"abc123"}"#).expect("JSON");
    assert!(started.is_state("STARTED"), "state matching is ASCII-cased");
    assert_eq!(
        started.sid.as_ref().and_then(json_text).as_deref(),
        Some("abc123")
    );

    let numeric = timer_state(br#"{"state":"started","sid":99887766}"#).expect("JSON");
    assert_eq!(
        numeric.sid.as_ref().and_then(json_text).as_deref(),
        Some("99887766")
    );
}

#[test]
fn timer_state_reports_none_for_a_non_json_body() {
    assert!(timer_state(b"<html>not json</html>").is_none());
}

#[test]
fn timer_state_text_falls_back_to_the_code_field() {
    let state = timer_state(br#"{"code":13}"#).expect("JSON");
    assert!(!state.is_state("done"));
    assert_eq!(state.state_text(), "13");
    let empty = timer_state(b"{}").expect("JSON");
    assert_eq!(empty.state_text(), "unknown");
}

#[test]
fn diagnose_prefers_a_page_error_message_over_the_title() {
    let html = r#"<title>Rapidgator</title><div class="error">File not found</div>"#;
    assert_eq!(diagnose(html), "page message: File not found");
    assert_eq!(
        diagnose("<title>Rapidgator - 404</title><body></body>"),
        "page \"Rapidgator - 404\" carries no free-download markers"
    );
    assert_eq!(
        diagnose("<body></body>"),
        "the response page carries no free-download markers"
    );
}

/// A multi-byte page must never panic a window-clamping parser.
#[test]
fn parsers_tolerate_multi_byte_characters() {
    let padding = "\u{e4}\u{f6}\u{fc}".repeat(200);
    let html = format!("<title>{padding}</title>Delay between downloads must be not less than");
    assert!(!diagnose(&html).is_empty());
    assert_eq!(ip_block_seconds(&html), None, "no digits follow the marker");
}

#[test]
fn encode_form_produces_url_encoded_pairs() {
    let encoded = encode_form(&[
        (
            "DownloadCaptchaForm[verifyCode]".to_owned(),
            "a b".to_owned(),
        ),
        ("g-recaptcha-response".to_owned(), "tok3n".to_owned()),
    ]);
    assert_eq!(
        String::from_utf8_lossy(&encoded),
        "DownloadCaptchaForm%5BverifyCode%5D=a+b&g-recaptcha-response=tok3n"
    );
}

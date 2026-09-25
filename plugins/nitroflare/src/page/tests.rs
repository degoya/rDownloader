//! Unit coverage for the free-flow page parsers. Every case here runs without a host, which is
//! why the parsing lives in [`crate::page`] rather than in either adapter.

use super::*;

const FILE_PAGE: &str = r#"<html><head><title>Nitroflare - release.rar</title></head><body>
<div id="CountDownTimer" data-timer="45" style="display:none"></div>
<div class="g-recaptcha" data-sitekey="6Lc-nitro-key"></div>
</body></html>"#;

#[test]
fn timer_start_accepts_the_answer_jd_expects() {
    assert_eq!(timer_start("1"), TimerStart::Started);
    assert_eq!(timer_start("  1\n"), TimerStart::Started);
}

/// The brief described the wait seconds as coming back from this call; JD reads them off the
/// file page instead. A numeric answer is still honoured, so a drifted site keeps working.
#[test]
fn timer_start_accepts_a_numeric_answer_as_a_countdown() {
    assert_eq!(timer_start("45"), TimerStart::Countdown(45));
}

#[test]
fn timer_start_rejects_anything_else() {
    assert_eq!(timer_start("0"), TimerStart::Unrecognized);
    assert_eq!(timer_start(""), TimerStart::Unrecognized);
    assert_eq!(
        timer_start("<html>Free downloading is not possible</html>"),
        TimerStart::Unrecognized
    );
}

#[test]
fn countdown_seconds_reads_the_file_pages_timer_attribute() {
    assert_eq!(countdown_seconds(FILE_PAGE), Some(45));
    assert_eq!(countdown_seconds("<div>no timer here</div>"), None);
}

#[test]
fn recaptcha_site_key_prefers_the_widget_attribute() {
    assert_eq!(
        recaptcha_site_key(FILE_PAGE).as_deref(),
        Some("6Lc-nitro-key")
    );
}

#[test]
fn recaptcha_site_key_falls_back_to_a_key_carried_in_the_script_url() {
    let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=6LcNitroAAAAAKBeQQE893"></script>"#;
    assert_eq!(
        recaptcha_site_key(html).as_deref(),
        Some("6LcNitroAAAAAKBeQQE893")
    );
    assert_eq!(recaptcha_site_key("<div>nothing</div>"), None);
}

#[test]
fn ip_block_seconds_parses_the_stated_wait_in_minutes() {
    let html =
        "Free downloading is not possible. You have to wait 70 minutes to download your next file.";
    assert_eq!(ip_block_seconds(html), Some(70 * 60));
}

#[test]
fn ip_block_seconds_maps_the_remaining_address_bound_notices() {
    assert_eq!(
        ip_block_seconds(
            "<div>Your ip has been blocked, if you think it is mistake contact us</div>"
        ),
        Some(30 * 60)
    );
    assert_eq!(
        ip_block_seconds("You can't use free download with a VPN / proxy turned on."),
        Some(60 * 60)
    );
    assert_eq!(
        ip_block_seconds("To continue this download please purchase premium or turn off your VPN."),
        Some(60 * 60)
    );
    assert_eq!(
        ip_block_seconds("Free download is currently unavailable due to overloading in the server"),
        Some(5 * 60)
    );
}

#[test]
fn ip_block_seconds_reports_a_limit_without_a_stated_duration() {
    assert_eq!(
        ip_block_seconds("<div>Free downloading is not possible.</div>"),
        Some(0)
    );
    assert_eq!(ip_block_seconds(FILE_PAGE), None);
}

#[test]
fn is_premium_only_matches_both_phrasings() {
    assert!(is_premium_only(
        "This file is available with premium key only"
    ));
    assert!(is_premium_only("This file is available with Premium only"));
    assert!(!is_premium_only(FILE_PAGE));
}

#[test]
fn is_wrong_captcha_matches_both_rejection_phrases() {
    assert!(is_wrong_captcha("The captcha wasn't entered correctly"));
    assert!(is_wrong_captcha("<b>You have to fill the captcha</b>"));
    assert!(!is_wrong_captcha(FILE_PAGE));
}

#[test]
fn direct_link_reads_a_quoted_provider_url() {
    let html =
        r#"<a href="https://cdn7.nitroflare.com/d/tok3n/release.rar" class="btn">Download</a>"#;
    assert_eq!(
        direct_link(html).as_deref(),
        Some("https://cdn7.nitroflare.com/d/tok3n/release.rar")
    );
}

#[test]
fn direct_link_falls_back_to_the_click_here_anchor() {
    let html = r#"<a href="https://s3.example-cdn.net/get/abc">Click here to download</a>"#;
    assert_eq!(
        direct_link(html).as_deref(),
        Some("https://s3.example-cdn.net/get/abc")
    );
}

/// The file page's own URL must never be mistaken for the download link.
#[test]
fn direct_link_ignores_the_file_page_url() {
    assert_eq!(
        direct_link(r#"<a href="https://nitroflare.com/view/ABCDEFGHIJ">back</a>"#),
        None
    );
}

#[test]
fn is_provider_host_accepts_only_the_manifests_domain() {
    assert!(is_provider_host("nitroflare.com"));
    assert!(is_provider_host("cdn7.nitroflare.com"));
    assert!(!is_provider_host("nitroflare.net"));
    assert!(!is_provider_host("evil-nitroflare.com"));
}

#[test]
fn file_names_come_from_the_disposition_then_the_url() {
    assert_eq!(
        file_name_from_disposition("attachment; filename=\"release.rar\"").as_deref(),
        Some("release.rar")
    );
    assert_eq!(file_name_from_disposition("attachment").as_deref(), None);
    let url: url::Url = "https://cdn7.nitroflare.com/d/tok/release.rar"
        .parse()
        .expect("URL");
    assert_eq!(url_file_name(&url).as_deref(), Some("release.rar"));
}

#[test]
fn diagnose_prefers_a_page_error_message_over_the_title() {
    let html = r#"<title>Nitroflare</title><div class="error">File doesn't exist</div>"#;
    assert_eq!(diagnose(html), "page message: File doesn't exist");
    assert_eq!(
        diagnose("<title>Nitroflare - 404</title>"),
        "page \"Nitroflare - 404\" carries no free-download markers"
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
    let html = format!("<title>{padding}</title>You have to wait");
    assert!(!diagnose(&html).is_empty());
    assert_eq!(ip_block_seconds(&html), None);
    assert_eq!(countdown_seconds(&html), None);
}

#[test]
fn encode_form_produces_url_encoded_pairs() {
    let encoded = encode_form(&[("method", "fetchDownload"), ("captcha", "tok 3n")]);
    assert_eq!(
        String::from_utf8_lossy(&encoded),
        "method=fetchDownload&captcha=tok+3n"
    );
}

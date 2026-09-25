//! The page reader against the live page of 2026-09-21 and the synthetic states.

use plugin_common::HttpResponse;

use super::{
    CaptchaKind, captcha_form, diagnose, direct_link, encode_form, has_malware_advisory,
    has_password_form, is_html, retry_seconds, threshold_seconds,
};

/// The public file page, captured 2026-09-21 and sanitised (token, security value, scripts).
const FILE_PAGE: &str = include_str!("../tests/fixtures/file-page-2026-09-21.html");
/// Synthetic: the reCAPTCHA form JD's `MediafireCom` handles; not seen live.
const CAPTCHA_RECAPTCHA: &str = include_str!("../tests/fixtures/file-page-captcha-recaptcha.html");
/// Synthetic: MediaFire's own checkbox form; not seen live.
const CAPTCHA_CHECKBOX: &str = include_str!("../tests/fixtures/file-page-captcha-checkbox.html");
/// Synthetic: the per-IP threshold page; not seen live.
const THRESHOLD: &str = include_str!("../tests/fixtures/file-page-threshold.html");
/// Synthetic: the password form; not seen live.
const PASSWORD: &str = include_str!("../tests/fixtures/file-page-password.html");
/// Synthetic: the malware advisory; not seen live.
const MALWARE: &str = include_str!("../tests/fixtures/file-page-malware.html");

#[test]
fn the_live_page_yields_the_download_button() {
    assert_eq!(
        direct_link(FILE_PAGE).as_deref(),
        Some("https://download2269.mediafire.com/redacted-token/ipnyzofjcwri357/test-10mb.bin")
    );
    assert!(!has_malware_advisory(FILE_PAGE));
    assert_eq!(threshold_seconds(FILE_PAGE), None);
    assert!(!has_password_form(FILE_PAGE));
    assert_eq!(retry_seconds(FILE_PAGE), None);
    // The live page mentions `form_captcha` twice, both times as a CSS selector.
    assert_eq!(captcha_form(FILE_PAGE), None);
}

#[test]
fn the_fallback_markers_are_read_and_a_foreign_host_is_refused() {
    assert_eq!(
        direct_link(r#"<script>kNO = "https://download1.mediafire.com/t/k/n.bin";</script>"#)
            .as_deref(),
        Some("https://download1.mediafire.com/t/k/n.bin")
    );
    assert_eq!(
        direct_link(
            r#"<a id="downloadButton" href="//download1.mediafirecdn.com/t/k/a&amp;b.bin">"#
        )
        .as_deref(),
        Some("https://download1.mediafirecdn.com/t/k/a&b.bin")
    );
    assert_eq!(
        direct_link("<p>see https://download7.mediafire.com/t/k/n.bin now</p>").as_deref(),
        Some("https://download7.mediafire.com/t/k/n.bin")
    );
    assert_eq!(
        direct_link(
            r#"<a id="downloadButton" href="https://evil.test/download1.mediafire.com/x">"#
        ),
        None
    );
    assert_eq!(
        direct_link(r#"<a id="downloadButton" href="http://download1.mediafire.com/x">"#),
        None
    );
    assert_eq!(
        direct_link(r#"<a id="downloadButton" href="https://www.mediafire.com/file/k">"#),
        None
    );
    assert_eq!(direct_link(""), None);
}

#[test]
fn every_synthetic_state_is_told_apart() {
    let recaptcha = captcha_form(CAPTCHA_RECAPTCHA).expect("a captcha form");
    assert_eq!(
        recaptcha.kind,
        CaptchaKind::Recaptcha {
            site_key: "6LcSyntheticSiteKey000000000000000000000".to_owned()
        }
    );
    assert!(
        recaptcha
            .fields
            .contains(&("mf_captcha_challenge".to_owned(), "synthetic".to_owned()))
    );
    assert_eq!(direct_link(CAPTCHA_RECAPTCHA), None);

    let checkbox = captcha_form(CAPTCHA_CHECKBOX).expect("a captcha form");
    assert_eq!(checkbox.kind, CaptchaKind::Checkbox);

    assert_eq!(
        captcha_form(
            r#"<form name="form_captcha"><input type="hidden" name="x" value="1"></form>"#
        )
        .map(|form| form.kind),
        Some(CaptchaKind::Unknown)
    );

    assert_eq!(threshold_seconds(THRESHOLD), Some(3600));
    assert_eq!(
        threshold_seconds("<p>Download Threshold Exceeded</p>"),
        Some(0)
    );
    assert!(has_password_form(PASSWORD));
    assert_eq!(direct_link(PASSWORD), None);
    assert!(has_malware_advisory(MALWARE));
    assert_eq!(
        retry_seconds("<p>Please retry your download again in 45 seconds.</p>"),
        Some(45)
    );
    assert_eq!(retry_seconds("<h1>Temporarily Unavailable</h1>"), Some(300));
}

#[test]
fn the_diagnosis_names_the_page_and_the_form_is_encoded() {
    assert_eq!(diagnose(FILE_PAGE), "page titled \"test-10mb\"".to_owned());
    assert_eq!(diagnose("<html></html>"), "page without a title");
    assert_eq!(
        encode_form(&[
            ("a b".to_owned(), "c&d".to_owned()),
            ("e".to_owned(), "f".to_owned())
        ]),
        b"a+b=c%26d&e=f"
    );
    let page = HttpResponse {
        status: 200,
        final_url: "https://www.mediafire.com/file/k".to_owned(),
        headers: vec![(
            "Content-Type".to_owned(),
            "text/html; charset=UTF-8".to_owned(),
        )],
        body: Vec::new(),
    };
    assert!(is_html(&page));
    let file = HttpResponse {
        status: 200,
        final_url: "https://download1.mediafire.com/t/k/n.bin".to_owned(),
        headers: vec![(
            "Content-Type".to_owned(),
            "application/octet-stream".to_owned(),
        )],
        body: vec![0],
    };
    assert!(!is_html(&file));
    let untyped = HttpResponse {
        status: 200,
        final_url: String::new(),
        headers: Vec::new(),
        body: b"  <!doctype html>".to_vec(),
    };
    assert!(is_html(&untyped));
}

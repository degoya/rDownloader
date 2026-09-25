//! `resolve` on the scripted host: the captcha forms, all synthetic — the file page carried
//! none on 2026-09-21. A form is answered once and once only, and never worked around.

use rd_core::FailureKind;
use rd_plugin_api::Resolver;

use super::{
    CAPTCHA_CHECKBOX, CAPTCHA_RECAPTCHA, DIRECT_URL, FILE_PAGE, FILE_URL, GET_INFO, MockHost, html,
    json, resolve_request, resolver,
};

/// A reCAPTCHA form goes to the host with the page it sits on and the answer is posted back.
#[tokio::test]
async fn a_recaptcha_form_is_handed_over_and_its_answer_posted() {
    let host = MockHost::solving(
        vec![
            json(200, GET_INFO),
            html(CAPTCHA_RECAPTCHA),
            html(FILE_PAGE),
        ],
        Some("solved-token"),
    );
    let resolved = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect("resolved after the captcha");
    assert_eq!(resolved.url.as_str(), DIRECT_URL);

    let captchas = host.captchas.lock().expect("mock lock");
    assert_eq!(captchas.len(), 1);
    let rd_plugin_api::CaptchaChallenge::RecaptchaV2(widget) = &captchas[0] else {
        panic!("expected reCAPTCHA v2: {:?}", captchas[0]);
    };
    assert_eq!(widget.site_key, "6LcSyntheticSiteKey000000000000000000000");
    assert_eq!(
        widget.page_url, FILE_URL,
        "the widget's page lies in the domains"
    );
    assert!(!widget.invisible);

    let requests = host.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].url.as_str(), FILE_URL);
    let body = String::from_utf8_lossy(&requests[2].body);
    assert!(body.contains("g-recaptcha-response=solved-token"), "{body}");
    assert!(body.contains("mf_captcha_challenge=synthetic"), "{body}");
}

#[tokio::test]
async fn the_checkbox_form_is_answered_with_the_flag_and_a_second_form_is_a_rejection() {
    let host = MockHost::with_responses(vec![
        json(200, GET_INFO),
        html(CAPTCHA_CHECKBOX),
        html(FILE_PAGE),
    ]);
    resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect("resolved after the checkbox");
    let requests = host.requests();
    assert_eq!(
        String::from_utf8_lossy(&requests[2].body),
        "mf_captcha_response=1"
    );
    assert!(host.captchas.lock().expect("mock lock").is_empty());

    let host = MockHost::with_responses(vec![
        json(200, GET_INFO),
        html(CAPTCHA_CHECKBOX),
        html(CAPTCHA_CHECKBOX),
    ]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("rejected");
    assert_eq!(failure.code.as_deref(), Some("mediafire.captcha_rejected"));
    assert_eq!(failure.category, FailureKind::CaptchaFailed);
    assert_eq!(host.requests().len(), 3, "answered once and once only");
}

#[tokio::test]
async fn an_unknown_captcha_form_and_a_host_without_a_solver_are_reported_not_worked_around() {
    let unknown =
        r#"<html><form name="form_captcha"><input type="hidden" name="x" value="1"></form></html>"#;
    let host = MockHost::with_responses(vec![json(200, GET_INFO), html(unknown)]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("unknown captcha");
    assert_eq!(failure.code.as_deref(), Some("mediafire.captcha_required"));
    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(host.requests().len(), 2, "nothing is posted blind");

    let host = MockHost::with_responses(vec![json(200, GET_INFO), html(CAPTCHA_RECAPTCHA)]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("no solver");
    assert_eq!(failure.code.as_deref(), Some("captcha.no_solver"));
    assert_eq!(host.requests().len(), 2);
}

//! Criterion 2: every measured refusal ends in exactly one code, and an answer that is a page
//! never reads as a result.

use plugin_common::FailureKind;

use super::{HITFILE, MockHost, TURBOBIT, code, hf, html, json, run, tb};
use crate::api::{self, Count, Refusal};

#[test]
fn the_measured_refusals_map_to_their_kinds() {
    for (status, body, expected) in [
        (404, tb::INFO_DELETED, Refusal::FileUnavailable),
        (404, hf::INFO_DELETED, Refusal::FileUnavailable),
        (404, tb::INIT_NOT_FOUND, Refusal::FileUnavailable),
        (404, tb::START_NOT_FOUND, Refusal::FileUnavailable),
        (404, hf::START_NOT_FOUND, Refusal::FileUnavailable),
        (422, tb::CAPTCHA_INVALID, Refusal::CaptchaInvalid),
        (422, hf::CAPTCHA_INVALID, Refusal::CaptchaInvalid),
        (409, hf::START_409, Refusal::NoDirectLink),
        (400, hf::START_400_FEASIBILITY, Refusal::PremiumOnly),
        (401, tb::USER_INFO_401, Refusal::Unauthenticated),
        (422, tb::LOGIN_NEED_CAPTCHA, Refusal::NeedCaptcha),
        (
            422,
            tb::LOGIN_PASSWORD_INCORRECT,
            Refusal::PasswordIncorrect,
        ),
        (422, tb::LOGIN_INVALID_CAPTCHA, Refusal::CaptchaInvalid),
        (
            400,
            r#"{"message":"File size is greater than allowed"}"#,
            Refusal::PremiumOnly,
        ),
        (429, "", Refusal::RateLimited),
        (503, "", Refusal::Http(503)),
        (
            400,
            r#"{"error_name":"something_else","data":[]}"#,
            Refusal::Api("something_else".to_owned()),
        ),
        (
            422,
            r#"{"message":"x","errors":{"fileId":["required"]}}"#,
            Refusal::Api("fileId".to_owned()),
        ),
        (
            422,
            r#"{"message":"x"}"#,
            Refusal::Api("validation".to_owned()),
        ),
        (
            400,
            r#"{"error_name":"<html>500</html>"}"#,
            Refusal::Api("refused".to_owned()),
        ),
    ] {
        assert_eq!(
            api::refusal(&json(status, body)),
            Some(expected),
            "{status} {body}"
        );
    }
    assert_eq!(api::refusal(&json(200, tb::INFO_FREE)), None);
    assert_eq!(api::refusal(&json(200, tb::INIT_DIRECT_HIT)), None);
}

#[test]
fn every_refusal_has_one_code_and_the_kind_the_scheduler_acts_on() {
    let cases: Vec<(Refusal, &str, FailureKind)> = vec![
        (
            Refusal::FileUnavailable,
            "turbobit.file_unavailable",
            FailureKind::Offline,
        ),
        (
            Refusal::PremiumOnly,
            "turbobit.premium_only",
            FailureKind::AuthRequired,
        ),
        (
            Refusal::NoDirectLink,
            "turbobit.no_direct_link",
            FailureKind::Permanent,
        ),
        (
            Refusal::CaptchaInvalid,
            "turbobit.captcha_rejected",
            FailureKind::CaptchaFailed,
        ),
        (
            Refusal::PasswordIncorrect,
            "turbobit.login_failed",
            FailureKind::AccountInvalid,
        ),
        (
            Refusal::NeedCaptcha,
            "turbobit.login_captcha",
            FailureKind::Transient(None),
        ),
        (
            Refusal::Unauthenticated,
            "turbobit.not_signed_in",
            FailureKind::AuthRequired,
        ),
        (
            Refusal::RateLimited,
            "turbobit.rate_limited",
            FailureKind::RateLimited(None),
        ),
        (
            Refusal::Api("x".to_owned()),
            "turbobit.api_error",
            FailureKind::Permanent,
        ),
        (
            Refusal::Http(503),
            "turbobit.http_error",
            FailureKind::Transient(None),
        ),
        (
            Refusal::Http(403),
            "turbobit.http_error",
            FailureKind::Permanent,
        ),
    ];
    for (refusal, expected_code, kind) in cases {
        let failure = api::failure_for(&TURBOBIT, refusal.clone());
        assert_eq!(code(&failure), expected_code, "{refusal:?}");
        assert_eq!(failure.kind, kind, "{refusal:?}");
        assert!(
            failure.message.starts_with("Turbobit: "),
            "{}",
            failure.message
        );
    }
    let failure = api::failure_for(&HITFILE, Refusal::Api("weird_code".to_owned()));
    assert_eq!(code(&failure), "hitfile.api_error");
    assert_eq!(
        failure.params,
        vec![("code".to_owned(), "weird_code".to_owned())]
    );
    let failure = api::failure_for(&HITFILE, Refusal::Http(502));
    assert_eq!(
        failure.params,
        vec![("status".to_owned(), "502".to_owned())]
    );
}

/// A provider's prose never becomes a parameter: only the `error_name` travels, and only when
/// it is shaped like a code.
#[test]
fn the_operators_message_never_travels_into_a_failure() {
    let failure = api::failure_for(
        &TURBOBIT,
        api::refusal(&json(
            400,
            r#"{"message":"Secret file /home/x/y.rar is gone"}"#,
        ))
        .expect("refused"),
    );
    assert!(!failure.message.contains("Secret file"));
    assert!(
        failure
            .params
            .iter()
            .all(|(_, value)| !value.contains("Secret"))
    );
}

#[test]
fn a_page_instead_of_json_is_a_permanent_read_failure_never_a_result() {
    for (status, page) in [
        (200, tb::SHELL),
        (200, hf::SHELL),
        (302, tb::REDIRECT_PAGE),
        (409, hf::ERROR_PAGE),
    ] {
        let host = MockHost::new(vec![html(status, page)]);
        let outcome: Result<Result<serde_json::Value, Refusal>, _> = run(api::exchange(
            &TURBOBIT,
            &host,
            api::get(&TURBOBIT, "captcha", "https://turbobit.net/"),
            "captcha",
        ));
        let failure = outcome.expect_err("a page is never an answer");
        assert_eq!(code(&failure), "turbobit.invalid_response", "{status}");
        assert_eq!(failure.kind, FailureKind::Permanent);
        assert_eq!(
            failure.params,
            vec![("field".to_owned(), "captcha".to_owned())]
        );
    }
    // The same without a content type: the body opens like markup.
    let mut bare = html(200, tb::SHELL);
    bare.headers.clear();
    let host = MockHost::new(vec![bare]);
    let outcome: Result<Result<serde_json::Value, Refusal>, _> = run(api::exchange(
        &HITFILE,
        &host,
        api::get(&HITFILE, "captcha", "https://hitfile.net/"),
        "captcha",
    ));
    assert_eq!(
        code(&outcome.expect_err("markup")),
        "hitfile.invalid_response"
    );
}

#[test]
fn every_request_carries_the_headers_the_api_answers_json_for() {
    let request = api::post(
        &TURBOBIT,
        "download/info",
        &serde_json::json!({"fileId": "a1b2c3d4e5f6"}),
        "https://turbobit.net/download/free/a1b2c3d4e5f6",
    );
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://app.turbobit.net/api/download/info");
    assert_eq!(
        super::header_of(&request, "accept"),
        Some("application/json")
    );
    assert_eq!(
        super::header_of(&request, "content-type"),
        Some("application/json")
    );
    assert_eq!(
        super::header_of(&request, "origin"),
        Some("https://turbobit.net")
    );
    assert_eq!(
        super::header_of(&request, "referer"),
        Some("https://turbobit.net/download/free/a1b2c3d4e5f6")
    );
    assert_eq!(super::body_of(&request), r#"{"fileId":"a1b2c3d4e5f6"}"#);

    let check = api::links_check(
        &HITFILE,
        &[
            "https://hitfile.net/Ab1CdEf".to_owned(),
            "https://hitfile.net/0ZGT".to_owned(),
        ],
    );
    assert_eq!(check.url, "https://app.hitfile.net/api/links/check");
    assert_eq!(
        super::header_of(&check, "content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        super::body_of(&check),
        "links=https%3A%2F%2Fhitfile.net%2FAb1CdEf%0Ahttps%3A%2F%2Fhitfile.net%2F0ZGT"
    );
}

#[test]
fn a_count_arrives_as_a_number_a_float_or_a_string_with_a_comma() {
    let parse = |text: &str| serde_json::from_str::<Count>(text).expect("count");
    assert_eq!(parse("946055308").into_u64(), Some(946_055_308));
    assert_eq!(parse("\"946055308\"").into_u64(), Some(946_055_308));
    assert_eq!(parse("12.7").into_u64(), Some(12));
    assert_eq!(parse("\"12,5\"").into_f64(), Some(12.5));
    assert_eq!(parse("\"abc\"").into_u64(), None);
    assert_eq!(parse("-1.0").into_u64(), None);
}

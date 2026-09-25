//! Unit coverage for the host-free pieces of the account-less flow: request bodies, the
//! `time_wait` reader, the wait/limit decisions and the captcha-image validation.

use super::*;

fn body_of(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf8 body")
}

fn parse(json: &str) -> FreeUrlResult {
    serde_json::from_str(json).expect("FreeUrlResult")
}

#[test]
fn the_probe_body_carries_only_the_file_id() {
    assert_eq!(
        body_of(geturl_probe_body("abcdefghijklm")),
        r#"{"file_id":"abcdefghijklm"}"#
    );
}

#[test]
fn the_requestcaptcha_body_is_an_empty_object() {
    // JD posts `new HashMap<String, Object>()`, not the file id — see the module doc.
    assert_eq!(body_of(requestcaptcha_body()), "{}");
}

#[test]
fn the_captcha_body_carries_the_challenge_and_the_typed_answer() {
    assert_eq!(
        body_of(geturl_captcha_body("abcdefghijklm", "chal-1", "TYPED")),
        r#"{"file_id":"abcdefghijklm","captcha_challenge":"chal-1","captcha_response":"TYPED"}"#
    );
}

/// The wait round drops the captcha fields, exactly as JD removes them from `postdata`.
#[test]
fn the_wait_round_body_carries_the_free_download_key_and_no_captcha_fields() {
    let body = body_of(geturl_free_key_body("abcdefghijklm", "homeHash"));
    assert_eq!(
        body,
        r#"{"file_id":"abcdefghijklm","free_download_key":"homeHash"}"#
    );
    assert!(!body.contains("captcha"));
}

#[test]
fn time_wait_is_read_as_a_number_or_a_numeric_string() {
    assert_eq!(parse(r#"{"time_wait":30}"#).wait_seconds(), Some(30));
    assert_eq!(
        parse(r#"{"time_wait":"45.000000"}"#).wait_seconds(),
        Some(45)
    );
    assert_eq!(parse(r#"{"time_wait":null}"#).wait_seconds(), None);
    assert_eq!(parse(r#"{"url":"https://k2s.cc/x"}"#).wait_seconds(), None);
}

#[test]
fn a_short_countdown_is_waited_out_and_a_long_one_blocks_the_ip() {
    assert_eq!(wait_step(30, 0), WaitStep::Wait(30));
    assert_eq!(wait_step(MAX_WAIT_SECONDS, 0), WaitStep::Wait(180));
    assert_eq!(wait_step(MAX_WAIT_SECONDS + 1, 0), WaitStep::Blocked(181));
    // JD stops after five rounds in a row rather than waiting a sixth time.
    assert_eq!(wait_step(30, MAX_WAIT_ROUNDS - 1), WaitStep::Wait(30));
    assert_eq!(wait_step(30, MAX_WAIT_ROUNDS), WaitStep::Blocked(30));
}

#[test]
fn a_stated_limit_becomes_an_ip_block_that_keeps_the_provider_wording() {
    let classified = super::super::classify_errorcode(2, "Traffic limit exceed", None);
    let failure = free_failure(classified);
    assert!(matches!(failure.kind, ErrorKind::IpBlocked(Some(3600))));
    assert_eq!(failure.code, messages::FREE_LIMIT_REACHED);
    assert!(
        failure
            .params
            .iter()
            .any(|(name, value)| *name == "message" && value == messages::TRAFFIC_EXHAUSTED.1)
    );
    assert!(
        failure
            .params
            .iter()
            .any(|(name, value)| *name == "wait_seconds" && value == "3600")
    );
}

/// Errorcode 5 carries the API's own `timeRemaining`, which must survive the re-labelling.
#[test]
fn a_stated_wait_keeps_its_parsed_seconds() {
    let classified = super::super::classify_errorcode(5, "Please wait", Some("2521.000000"));
    let failure = free_failure(classified);
    assert!(matches!(failure.kind, ErrorKind::IpBlocked(Some(2521))));
}

/// Anything that is not a limit passes through untouched — a premium-only file must stay an
/// `AuthRequired`, not become an IP block that holds back every other free link.
#[test]
fn a_non_limit_failure_is_left_alone() {
    let classified = super::super::classify_errorcode(7, "Premium only", None);
    let failure = free_failure(classified);
    assert!(matches!(failure.kind, ErrorKind::AuthRequired));
    assert_eq!(failure.code, messages::PREMIUM_REQUIRED.0);
}

#[test]
fn errorcode_thirty_is_recognized_as_a_captcha_demand() {
    assert!(needs_captcha(&super::super::classify_errorcode(
        30,
        "Send captcha fields",
        None
    )));
    assert!(!needs_captcha(&super::super::classify_errorcode(
        31,
        "Wrong captcha",
        None
    )));
    assert!(!needs_captcha(&super::super::classify_errorcode(
        20,
        "File not found",
        None
    )));
}

#[test]
fn the_captcha_url_is_resolved_and_upgraded_to_https() {
    let result = RequestCaptchaResult {
        challenge: Some("chal-1".to_owned()),
        captcha_url: Some("http://k2s.cc/api/v2/captcha.html?id=chal-1".to_owned()),
    };
    let (challenge, image) = captcha_request(&result).expect("challenge");
    assert_eq!(challenge, "chal-1");
    assert_eq!(
        image.as_str(),
        "https://k2s.cc/api/v2/captcha.html?id=chal-1"
    );
}

#[test]
fn a_relative_captcha_url_is_resolved_against_the_api_base() {
    let result = RequestCaptchaResult {
        challenge: Some("chal-1".to_owned()),
        captcha_url: Some("/captcha/chal-1.png".to_owned()),
    };
    let (_, image) = captcha_request(&result).expect("challenge");
    assert_eq!(image.as_str(), "https://k2s.cc/captcha/chal-1.png");
}

#[test]
fn a_missing_challenge_or_image_is_reported_rather_than_guessed() {
    for result in [
        RequestCaptchaResult {
            challenge: None,
            captcha_url: Some("https://k2s.cc/c.png".to_owned()),
        },
        RequestCaptchaResult {
            challenge: Some("chal-1".to_owned()),
            captcha_url: Some("  ".to_owned()),
        },
    ] {
        let failure = captcha_request(&result).expect_err("incomplete challenge");
        assert_eq!(failure.code, messages::CAPTCHA_UNAVAILABLE.0);
    }
}

#[test]
fn the_image_type_comes_from_the_header_and_falls_back_to_the_bytes() {
    assert_eq!(
        image_mime(Some("image/JPEG; charset=binary"), b"").expect("declared"),
        "image/jpeg"
    );
    assert_eq!(
        image_mime(None, b"\x89PNG\r\n\x1a\nrest").expect("sniffed"),
        "image/png"
    );
    assert_eq!(
        image_mime(Some("application/octet-stream"), b"\xff\xd8\xffrest").expect("sniffed"),
        "image/jpeg"
    );
}

/// An HTML error page served where the captcha should be must fail, not reach a solver as an
/// unreadable "image".
#[test]
fn a_body_that_is_not_an_image_is_reported() {
    let failure = image_mime(Some("text/html"), b"<html>error</html>").expect_err("not an image");
    assert_eq!(failure.code, messages::CAPTCHA_UNAVAILABLE.0);
}

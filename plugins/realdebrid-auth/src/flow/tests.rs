//! What the endpoint answers mean, checked without a host or a component.

use super::*;

#[test]
fn a_granted_exchange_yields_the_three_values_the_host_stores() {
    let answer = read_token_answer(
        200,
        None,
        r#"{"access_token":"AT","expires_in":3600,"token_type":"Bearer","refresh_token":"RT"}"#,
    );
    assert_eq!(
        answer,
        TokenAnswer::Granted {
            access_token: "AT".to_owned(),
            refresh_token: Some("RT".to_owned()),
            expires_in_seconds: Some(3600),
        }
    );
}

/// The difference between a sign-in that finishes and one that dies while the person is still
/// reading the code off the screen.
#[test]
fn an_unconfirmed_device_code_keeps_the_flow_open() {
    assert_eq!(
        read_token_answer(400, None, r#"{"error":"authorization_pending"}"#),
        TokenAnswer::Busy(5)
    );
    assert_eq!(
        read_token_answer(
            400,
            None,
            r#"{"error":"authorization_pending","interval":7}"#
        ),
        TokenAnswer::Busy(7)
    );
}

/// Real-Debrid's own "slow down" is a number, not a word, and it counts towards the very cap
/// that produced it — so it has to be read as a wait rather than as a refusal.
#[test]
fn the_provider_numeric_wait_codes_are_waits() {
    for api_code in [5, 34] {
        assert_eq!(
            read_token_answer(400, None, &format!(r#"{{"error_code":{api_code}}}"#)),
            TokenAnswer::Busy(5),
            "{api_code}"
        );
    }
    assert_eq!(
        read_token_answer(429, Some("90"), r#"{"error":"slow_down","interval":5}"#),
        TokenAnswer::Busy(90)
    );
}

#[test]
fn a_refused_consent_is_a_refusal_and_not_a_wait() {
    assert_eq!(
        read_token_answer(400, None, r#"{"error":"access_denied"}"#),
        TokenAnswer::Refused {
            error: "access_denied".to_owned(),
            api_code: None,
        }
    );
    assert_eq!(refusal_code("access_denied", None), "consent_denied");
}

#[test]
fn an_expired_code_and_a_refused_renewal_are_told_apart() {
    assert_eq!(refusal_code("expired_token", None), "code_expired");
    assert_eq!(refusal_code("invalid_grant", None), "code_expired");
    assert_eq!(refusal_code("", Some(8)), "sign_in_refused");
}

/// The refusal that is about this build rather than about the person. Sending somebody back to
/// sign in again on `invalid_client` would be a loop with no end in it.
#[test]
fn a_rejected_application_registration_gets_its_own_code() {
    assert_eq!(refusal_code("invalid_client", None), "client_rejected");
    assert_eq!(refusal_code("unauthorized_client", None), "client_rejected");
}

#[test]
fn a_second_factor_is_told_apart_from_a_wrong_credential() {
    assert_eq!(refusal_code("", Some(10)), "two_factor");
    assert_eq!(refusal_code("", Some(11)), "two_factor");
    assert_eq!(refusal_code("", Some(14)), "sign_in_refused");
}

/// A number with no word is still a refusal. Reading only `error` would take one of those for
/// a success with no token in it.
#[test]
fn a_numeric_refusal_without_a_word_still_refuses() {
    assert_eq!(
        read_token_answer(403, None, r#"{"error_code":9}"#),
        TokenAnswer::Refused {
            error: String::new(),
            api_code: Some(9),
        }
    );
}

#[test]
fn an_answer_without_a_token_is_never_read_as_success() {
    assert_eq!(
        read_token_answer(200, None, r#"{"token_type":"Bearer"}"#),
        TokenAnswer::Unreadable(200)
    );
    assert_eq!(
        read_token_answer(200, None, r#"{"access_token":""}"#),
        TokenAnswer::Unreadable(200)
    );
}

#[test]
fn a_device_authorization_answer_is_read_whole() {
    let body = r#"{"device_code":"DC-1","user_code":"WXYZ1234",
      "verification_url":"https:\/\/real-debrid.com\/device",
      "expires_in":1800,"interval":5,
      "direct_verification_url":"https:\/\/real-debrid.com\/device?code=WXYZ1234"}"#;
    assert_eq!(
        read_device_code(body),
        Some(DeviceCode {
            device_code: "DC-1".to_owned(),
            user_code: "WXYZ1234".to_owned(),
            verification_url: "https://real-debrid.com/device".to_owned(),
            expires_in: Some(1800),
            interval: Some(5),
        })
    );
    // The RFC 8628 spelling, and the only other one accepted.
    let rfc = r#"{"device_code":"DC-1","user_code":"WX",
      "verification_uri":"https:\/\/real-debrid.com\/device"}"#;
    assert_eq!(
        read_device_code(rfc).map(|code| code.verification_url),
        Some("https://real-debrid.com/device".to_owned())
    );
    // A prompt nobody could act on is not a prompt.
    assert_eq!(read_device_code(r#"{"device_code":"DC-1"}"#), None);
    assert_eq!(
        read_device_code(r#"{"device_code":"","user_code":"WX","verification_url":"https://x"}"#),
        None
    );
}

/// The guard the whole sanitising rule exists for: an endpoint that echoes a token into its
/// error document must publish none of it.
#[test]
fn a_providers_error_text_never_travels_verbatim() {
    assert_eq!(sanitize_error("invalid_grant"), "invalid_grant");
    assert_eq!(sanitize_error("token AT-7f3c9 rejected"), "refused");
    assert_eq!(sanitize_error("<html>500</html>"), "refused");
    assert_eq!(sanitize_error(""), "refused");
    assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
}

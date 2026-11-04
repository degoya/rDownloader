//! The captcha handlers' own rules: the polling client and the solver endpoint.

use super::{CaptureCaptchaClient, ListCaptureCaptchasQuery, validated_endpoint};

/// The extension names itself in the query string; an agent that says nothing is still
/// served, and a client nobody knows is refused rather than silently counted as one.
#[test]
fn the_polling_client_is_optional_and_only_known_names_are_accepted() {
    let named: ListCaptureCaptchasQuery =
        serde_json::from_value(serde_json::json!({ "client": "browser_extension" }))
            .expect("known client");
    assert_eq!(named.client, Some(CaptureCaptchaClient::BrowserExtension));

    let unnamed: ListCaptureCaptchasQuery =
        serde_json::from_value(serde_json::json!({})).expect("no client");
    assert_eq!(unnamed.client, None);

    assert!(
        serde_json::from_value::<ListCaptureCaptchasQuery>(
            serde_json::json!({ "client": "toaster" })
        )
        .is_err(),
        "an unknown client must not pass as the extension"
    );
}

/// The endpoint receives the API key, so it must be https and absolute — or http where the
/// configuration allows plain http (RD-1420-01).
#[test]
fn only_absolute_https_endpoints_are_accepted() {
    assert_eq!(
        validated_endpoint("https://api.capmonster.cloud/", false).expect("https"),
        "https://api.capmonster.cloud"
    );
    assert_eq!(
        validated_endpoint("  ", false).expect("blank falls back"),
        rd_captcha::DEFAULT_ENDPOINT
    );
    for rejected in [
        "http://api.2captcha.com",
        "api.2captcha.com",
        "ftp://example.test",
        "https://",
    ] {
        assert!(
            validated_endpoint(rejected, false).is_err(),
            "{rejected} must be rejected"
        );
    }
    assert_eq!(
        validated_endpoint("http://solver.lan:8000/", true).expect("allowed"),
        "http://solver.lan:8000"
    );
}

//! The solver endpoint and its configuration's codes (RD-1420-01, audit L-08): plain http is
//! refused with a code of its own unless the configuration allows it, and then stored and tried
//! as typed.

use axum::http::StatusCode;

use crate::common::test_harness;

#[tokio::test]
async fn a_malformed_configuration_is_refused_with_a_code_the_ui_can_translate() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;

    let (status, error) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "endpoint": "http://api.2captcha.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error["code"], "captcha.endpoint_plain_http",
        "a plain-HTTP endpoint would put the key on the wire"
    );

    let (status, error) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "api_key": "   " }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.api_key_invalid");
}

#[tokio::test]
async fn a_plain_http_endpoint_is_kept_only_while_the_setting_allows_it() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;

    let (status, config) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "endpoint": "http://solver.lan:8000/", "allow_plain_http": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["endpoint"], "http://solver.lan:8000");
    assert_eq!(config["allow_plain_http"], true);
    let (_, stored) = harness.get("/api/v1/captcha-config").await;
    assert_eq!(stored["allow_plain_http"], true);

    // Switching it off under the stored http endpoint would leave a configuration the broker
    // refuses; the switch is refused instead.
    let (status, error) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "allow_plain_http": false }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.endpoint_plain_http");

    // The test button sends the form's setting along: off, a typed http endpoint is refused.
    let (status, error) = harness
        .send(
            "POST",
            "/api/v1/captcha-config/test",
            serde_json::json!({
                "endpoint": "http://solver.lan:8000",
                "api_key": "k",
                "allow_plain_http": false,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.endpoint_plain_http");

    let (status, config) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({
                "endpoint": "https://api.capmonster.cloud",
                "allow_plain_http": false,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["allow_plain_http"], false);
}

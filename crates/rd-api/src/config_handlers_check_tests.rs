//! How a failed account check reads to the interface (RD-120-45).

use axum::response::IntoResponse;
use rd_core::{Failure, FailureKind};

use super::account_check_failed;

async fn body(failure: Failure) -> serde_json::Value {
    let response = account_check_failed(failure).into_response();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

/// The sign-in stopped because the browser showed no widget: the interface gets that code and
/// the hoster, which it translates into what to do — not an English sentence inside
/// `account.check_failed`.
#[tokio::test]
async fn a_page_without_its_widget_keeps_its_code_and_host() {
    let failure = Failure::coded(
        FailureKind::NeedsCaptcha,
        "captcha.page_without_widget",
        "ddownload.com showed no captcha in your browser",
    )
    .with_param("host", "ddownload.com");

    let body = body(failure).await;

    assert_eq!(body["code"], "captcha.page_without_widget");
    assert_eq!(body["params"]["host"], "ddownload.com");
}

/// Everything else stays as it was: a plugin's own wording, quoted as the reason.
#[tokio::test]
async fn any_other_failure_is_quoted_as_the_reason() {
    let failure = Failure::coded(
        FailureKind::AccountInvalid,
        "ddownload.login_failed",
        "DDownload rejected the login",
    );

    let body = body(failure).await;

    assert_eq!(body["code"], "account.check_failed");
    assert_eq!(body["params"]["reason"], "DDownload rejected the login");
}

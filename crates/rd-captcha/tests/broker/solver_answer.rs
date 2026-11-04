//! The solver's answer is read with a size limit, and its endpoint is https unless the setting
//! allows plain http (beside `broker.rs`, which is at its length cap).

use super::*;

#[tokio::test]
async fn an_oversized_solver_answer_is_not_read_to_the_end() {
    let fixture = fixture().await;
    // Valid JSON, but far larger than any answer a solver gives.
    let padding = "x".repeat(2 * 1024 * 1024);
    let router = Router::new().route(
        "/getBalance",
        post(async move || format!(r#"{{"errorId":0,"balance":1.0,"padding":"{padding}"}}"#)),
    );
    let server = solver_service(router).await;

    let failure = fixture
        .broker
        .test_solver_with(Some(server.url("")), Some("key".to_owned()), Some(true))
        .await
        .expect_err("answer too large");
    assert_eq!(failure.code.as_deref(), Some("captcha.solver_failed"));
    assert_eq!(failure.category, FailureKind::CaptchaFailed);
}

/// RD-1420-01 (audit L-08): a plain-http endpoint is refused with its code while the setting is
/// off — the service never sees the key — and used once the setting allows it.
#[tokio::test]
async fn a_plain_http_endpoint_is_used_only_when_the_setting_allows_it() {
    let fixture = fixture().await;
    let calls = SolverCalls::default();
    let server = solver_service(balance_service(
        calls.clone(),
        json!({ "errorId": 0, "balance": 2.0 }),
    ))
    .await;
    let reference = fixture.store_key("stored-key").await;
    let settings = json!({
        "solver": "two_captcha_compatible",
        "endpoint": server.url(""),
        "api_key_ref": reference,
    });
    fixture
        .database
        .set_setting(rd_captcha::SETTINGS_KEY.to_owned(), settings.clone())
        .await
        .expect("settings stored");

    let refused = fixture
        .broker
        .test_solver(None, None)
        .await
        .expect_err("plain http refused");
    assert_eq!(refused.code.as_deref(), Some("captcha.endpoint_plain_http"));
    assert_eq!(refused.category, FailureKind::Permanent);
    let typed = fixture
        .broker
        .test_solver_with(Some(server.url("")), Some("typed".to_owned()), Some(false))
        .await
        .expect_err("plain http refused for a typed endpoint too");
    assert_eq!(typed.code.as_deref(), Some("captcha.endpoint_plain_http"));
    let widget = fixture
        .broker
        .solve_within(widget(), AMPLE)
        .await
        .expect_err("no solving over plain http");
    assert_eq!(widget.code.as_deref(), Some("captcha.endpoint_plain_http"));
    assert!(calls.requests().is_empty(), "the key never left");

    fixture.configure(settings).await;
    let balance = fixture
        .broker
        .test_solver(None, None)
        .await
        .expect("allowed");
    assert!((balance - 2.0).abs() < f64::EPSILON);
    assert_eq!(calls.requests().len(), 1);
}

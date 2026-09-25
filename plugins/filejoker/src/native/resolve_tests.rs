//! `resolve()` tests, split out of `tests.rs` to keep both files under the crate layout's 500-line
//! convention; same `MockHost` harness and fixtures (re-used via `super::`).

use std::sync::Arc;

use rd_core::FailureKind;
use rd_plugin_api::{HostHttpResponse, ResolvedHeader, Resolver, ResolverHost};

use super::super::FilejokerResolver;
use super::{CAPTCHA_FORM_PAGE, FORM_PAGE, MockHost, file, html, resolve_request};

#[tokio::test]
async fn download_form_is_posted_and_redirect_target_is_used() {
    let host = MockHost::with_responses(vec![
        html(FORM_PAGE),
        file("https://fs1.filejoker.net/d/r4nd/release.rar"),
    ]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs1.filejoker.net/d/r4nd/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://filejoker.net/abc123xyz/release.rar"
    );
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].body,
        b"op=download2&id=abc123xyz&rand=r4nd&method_premium=Premium+Download"
    );
    assert!(requests[1].headers.iter().any(|header| {
        header.name == "Content-Type"
            && header.value_template == "application/x-www-form-urlencoded"
    }));
}

#[tokio::test]
async fn direct_link_page_after_post_is_followed() {
    let host = MockHost::with_responses(vec![
        html(FORM_PAGE),
        html(
            r#"<a href="https://filejoker.net/premium">Premium</a><a href="https://fs1.filejoker.net/d/r4nd/release.rar">Download File</a>"#,
        ),
        file("https://fs1.filejoker.net/d/r4nd/release.rar"),
    ]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs1.filejoker.net/d/r4nd/release.rar"
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 3);
}

#[tokio::test]
async fn direct_file_response_with_no_form_is_returned_as_is() {
    let response = HostHttpResponse {
        status: 206,
        final_url: "https://cdn.filejoker.net/file.bin".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Disposition".to_owned(),
            value: "attachment; filename=release.bin".to_owned(),
        }],
        body: vec![0],
    };
    let resolver = FilejokerResolver::new(MockHost::new(response));
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
}

#[tokio::test]
async fn login_wall_page_reports_session_invalid_with_diagnosis() {
    // The sign-in form, not the header's `/login` link: the link is on every guest page.
    let host = MockHost::with_responses(vec![html(
        r#"<form method="POST" name="FL"><input type="hidden" name="op" value="login"></form>"#,
    )]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("login wall must fail");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("filejoker.session_invalid"));
    assert_eq!(
        failure.params.get("diagnosis").map(String::as_str),
        Some(
            "the page requires a login - the cookies were not sent or do not belong to a logged-in session"
        )
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[tokio::test]
async fn offline_page_reports_offline() {
    let host = MockHost::with_responses(vec![html("<title>Error</title><b>File Not Found</b>")]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline file must fail");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("filejoker.file_offline"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[tokio::test]
async fn premium_only_page_after_post_reports_no_premium_file() {
    let host = MockHost::with_responses(vec![
        html(FORM_PAGE),
        html(r#"<div class="premium-download-expand">Premium members only</div>"#),
    ]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("premium-only page must fail");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("filejoker.no_premium_file"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 2);
}

#[tokio::test]
async fn wait_page_reports_transient_with_retry_after_seconds() {
    let host = MockHost::with_responses(vec![html(
        "<p>Please Wait <b>45</b> seconds before next download</p>",
    )]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("wait page must fail");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(45)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("filejoker.download_wait"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[tokio::test]
async fn unrecognized_page_with_no_form_reports_page_error() {
    let host = MockHost::with_responses(vec![html("<title>Something else</title>")]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("unrecognized page must fail");
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("filejoker.page_error"));
}

#[tokio::test]
async fn captcha_challenge_on_file_page_is_reported_and_form_is_not_posted() {
    let host = MockHost::with_responses(vec![html(CAPTCHA_FORM_PAGE)]);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("captcha challenge must fail");
    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("filejoker.captcha_required"));
    // Only the initial GET was made; the form was never posted.
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

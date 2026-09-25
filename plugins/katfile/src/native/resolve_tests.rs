//! `resolve()` tests, split out of `tests.rs` to keep both files under the crate layout's
//! 500-line convention; same `MockHost` harness and fixtures (re-used via `pub(crate)` items).

use std::sync::Arc;

use rd_core::AccountId;
use rd_plugin_api::{
    ClientIdentity, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver, ResolverHost,
};

use super::super::KatfileResolver;
use super::{
    CAPTCHA_FORM_PAGE, CAPTCHA_OUTSIDE_FORM_PAGE, FORM_PAGE, MockHost, file, html, json,
    resolve_request,
};

#[tokio::test]
async fn cookie_probe_returns_final_transfer_url() {
    let response = HostHttpResponse {
        status: 206,
        final_url: "https://cdn.katfile.biz/file.bin".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Disposition".to_owned(),
            value: "attachment; filename=release.bin".to_owned(),
        }],
        body: vec![0],
    };
    let resolver = KatfileResolver::new(MockHost::new(response, false));
    let account = AccountId::new();
    let resolved = resolver
        .resolve(ResolveRequest {
            url: "https://katfile.com/abc123xyz".parse().expect("URL"),
            client: ClientIdentity {
                account_id: Some(account),
                proxy_profile_id: None,
                tls_revision: 4,
            },
        })
        .await
        .expect("resolved");
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.client.account_id, Some(account));
}

#[tokio::test]
async fn download_form_is_posted_and_redirect_target_is_used() {
    let host = MockHost::with_responses(
        vec![
            html(FORM_PAGE),
            file("https://fs7.katfile.biz/d/r4nd/release.rar"),
        ],
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.katfile.biz/d/r4nd/release.rar"
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2);
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
    let host = MockHost::with_responses(
        vec![
            html(FORM_PAGE),
            html(
                r#"<a href="https://katfile.biz/premium">Premium</a><a href="https://fs7.katfile.biz/d/r4nd/release.rar">Download</a>"#,
            ),
            file("https://fs7.katfile.biz/d/r4nd/release.rar"),
        ],
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.katfile.biz/d/r4nd/release.rar"
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 3);
}

#[tokio::test]
async fn captcha_challenge_on_file_page_is_reported_and_form_is_not_posted() {
    let host = MockHost::with_responses(vec![html(CAPTCHA_FORM_PAGE)], false);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("captcha challenge must fail");
    assert_eq!(failure.category, rd_core::FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("katfile.captcha_required"));
    // Only the initial GET was made; the form was never posted.
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[tokio::test]
async fn captcha_widget_outside_the_form_does_not_block_resolve() {
    // Finding 3: has_captcha_challenge must be scoped to the download2 form, not the whole page —
    // a widget elsewhere (a login modal here) must not abort a resolve that would otherwise
    // succeed.
    let host = MockHost::with_responses(
        vec![
            html(CAPTCHA_OUTSIDE_FORM_PAGE),
            file("https://fs7.katfile.biz/d/r4nd/release.rar"),
        ],
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver
        .resolve(resolve_request())
        .await
        .expect("an unrelated captcha widget must not block the resolve");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.katfile.biz/d/r4nd/release.rar"
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 2);
}

#[tokio::test]
async fn premium_only_page_reports_auth_required_and_form_is_not_posted() {
    let host = MockHost::with_responses(
        vec![html(
            "<title>File</title><div>This file is available for Premium members only.</div>",
        )],
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("premium-only page must fail");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("katfile.premium_only"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[tokio::test]
async fn wait_page_reports_transient_with_retry_after_seconds() {
    // `var estimated_time = 450` is in TENTHS of a second (JD's own comment) -> 45 whole seconds.
    let host = MockHost::with_responses(
        vec![html(
            "<html><body><script>var estimated_time = 450;</script></body></html>",
        )],
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("wait page must fail");
    assert_eq!(
        failure.category,
        rd_core::FailureKind::Transient {
            retry_after_seconds: Some(45)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("katfile.download_wait"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[tokio::test]
async fn guest_page_after_post_reports_missing_premium_session() {
    let host = MockHost::with_responses(
        vec![html(FORM_PAGE), html("<html>please wait 60 seconds</html>")],
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("guest session");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("katfile.no_premium_file"));
    assert!(
        failure
            .message
            .starts_with("KatFile cookie session did not return a premium file:"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn resolve_without_secret_or_cookies_makes_no_requests() {
    let host = MockHost::bare(false, false);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing credentials must fail");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("katfile.cookie_session_required_for_download")
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 0);
}

#[tokio::test]
async fn api_key_without_cookie_session_cannot_download() {
    let host = MockHost::bare(true, false);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("cookies required");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("katfile.cookie_session_required_for_download")
    );
    assert!(
        failure.message.contains("cookie session"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn api_direct_link_is_used_without_cookies() {
    let host = MockHost::full(
        vec![HostHttpResponse {
            status: 200,
            final_url: "https://katfile.biz/api/file/direct_link"
                .parse()
                .expect("URL"),
            headers: Vec::new(),
            body: br#"{"status":200,"msg":"OK","result":{"url":"https://fs9.katfile.biz/d/tok/release.rar","size":"4096"}}"#.to_vec(),
        }],
        true,
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver
        .resolve(resolve_request())
        .await
        .expect("resolved via API");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs9.katfile.biz/d/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .url
            .as_str()
            .starts_with("https://katfile.biz/api/")
    );
    assert!(
        requests[0]
            .query
            .iter()
            .any(|q| q.name == "key" && q.value_template == "{{secret:katfile_api_key}}")
    );
}

#[tokio::test]
async fn file_info_reports_file_unavailable_for_non_200_item_status() {
    let host = MockHost::with_responses(
        vec![
            // `api_direct_link` attempt: no usable result -> silently falls through.
            json(
                "https://katfile.biz/api/file/direct_link",
                br#"{"status":404,"msg":"no file"}"#,
            ),
            // `file/info`: the single item reports the file is gone.
            json(
                "https://katfile.biz/api/file/info",
                br#"{"status":200,"msg":"OK","result":[{"status":404,"filecode":"abc123xyz"}]}"#,
            ),
        ],
        true,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline file must fail");
    assert_eq!(failure.category, rd_core::FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("katfile.file_unavailable"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 2);
}

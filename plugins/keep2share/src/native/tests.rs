use std::sync::Arc;

use rd_core::{AccountId, FailureKind};
use rd_plugin_api::test_support::ScriptedHost as MockHost;
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver,
    ResolverHost,
};

use super::Keep2ShareResolver;

fn json_response(status: u16, url: &str, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: url.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn login_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://k2s.cc/api/v2/login", body)
}

fn geturl_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://k2s.cc/api/v2/geturl", body)
}

fn accountinfo_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://k2s.cc/api/v2/accountinfo", body)
}

fn getfilesinfo_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://k2s.cc/api/v2/getfilesinfo", body)
}

const FILE_ID: &str = "abcdefghijklm"; // 13 chars, JD's minimum
const OFFLINE_ID: &str = "offline123456"; // 13 chars
const FILE_URL: &str = "https://k2s.cc/file/abcdefghijklm/release.html";
const LOGIN_OK: &str = r#"{"status":"success","code":200,"auth_token":"tok-abc"}"#;

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: FILE_URL.parse().expect("URL"),
        client: client_identity(),
    }
}

fn client_identity() -> ClientIdentity {
    ClientIdentity {
        account_id: Some(AccountId::new()),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn header_value<'a>(request: &'a HostHttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value_template.as_str())
}

fn body_str(request: &HostHttpRequest) -> &str {
    std::str::from_utf8(&request.body).expect("utf8 body")
}

#[tokio::test]
async fn happy_path_resolve_logs_in_then_calls_geturl_with_the_token() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            geturl_response(
                r#"{"status":"success","code":200,"url":"https://k2s.cc/d/tok-abc/release.rar"}"#,
            ),
        ],
        true,
    );
    let resolver = Keep2ShareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://k2s.cc/d/tok-abc/release.rar"
    );
    assert_eq!(resolved.file_name, None);
    assert_eq!(resolved.size, None);
    assert_eq!(resolved.checksum, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2);

    // Exact request assertions, including the literal template markers and JSON body bytes —
    // this is the host's JSON-body secret-expansion feature (Task 2) this plugin exercises.
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].url.as_str(), "https://k2s.cc/api/v2/login");
    assert_eq!(
        header_value(&requests[0], "content-type"),
        Some("application/json")
    );
    assert_eq!(
        body_str(&requests[0]),
        r#"{"username":"{{username}}","password":"{{secret:keep2share_password}}"}"#
    );

    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].url.as_str(), "https://k2s.cc/api/v2/geturl");
    assert_eq!(
        header_value(&requests[1], "content-type"),
        Some("application/json")
    );
    assert_eq!(
        body_str(&requests[1]),
        r#"{"file_id":"abcdefghijklm","auth_token":"tok-abc"}"#
    );
}

#[tokio::test]
async fn resolve_without_password_reports_password_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = Keep2ShareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("keep2share.password_missing"));
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn resolve_with_wrong_password_reports_account_invalid() {
    let host = MockHost::new(
        login_response(
            r#"{"status":"error","code":406,"message":"Invalid login or password","errorCode":70}"#,
        ),
        true,
    );
    let resolver = Keep2ShareResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("wrong password");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("keep2share.bad_credentials"));
}

#[tokio::test]
async fn resolve_login_captcha_demanded_reports_needs_captcha() {
    let host = MockHost::new(
        login_response(r#"{"status":"error","code":406,"errorCode":30}"#),
        true,
    );
    let resolver = Keep2ShareResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("captcha demanded");
    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("keep2share.login_captcha"));
}

#[tokio::test]
async fn resolve_offline_file_reports_offline() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            geturl_response(
                r#"{"status":"error","code":406,"errorCode":20,"message":"File not found"}"#,
            ),
        ],
        true,
    );
    let resolver = Keep2ShareResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("keep2share.file_offline"));
}

#[tokio::test]
async fn resolve_traffic_exhausted_reports_rate_limited_with_1hr_retry() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            geturl_response(
                r#"{"status":"error","code":406,"errorCode":2,"message":"Traffic limit exceed"}"#,
            ),
        ],
        true,
    );
    let resolver = Keep2ShareResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("traffic exhausted");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("keep2share.traffic_exhausted")
    );
}

#[tokio::test]
async fn check_account_reports_premium_traffic_and_expiry_label() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            accountinfo_response(
                r#"{"status":"success","code":200,"available_traffic":2048,"account_expires":1798761600}"#,
            ),
        ],
        true,
    );
    let resolver = Keep2ShareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.premium_until(until=2027-01-01)"
    );
    assert_eq!(status.traffic_left.map(|bytes| bytes.get()), Some(2048));

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].url.as_str(),
        "https://k2s.cc/api/v2/accountinfo"
    );
    assert_eq!(body_str(&requests[1]), r#"{"auth_token":"tok-abc"}"#);
}

#[tokio::test]
async fn check_account_never_premium_account_reports_free_and_no_traffic_expiry() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            accountinfo_response(
                r#"{"status":"success","code":200,"available_traffic":10737418240,"account_expires":false}"#,
            ),
        ],
        true,
    );
    let resolver = Keep2ShareResolver::new(host as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(!status.premium);
    assert_eq!(plugin_common::native::label_summary(&status.label), "");
    assert_eq!(
        status.traffic_left.map(|bytes| bytes.get()),
        Some(10_737_418_240)
    );
}

#[tokio::test]
async fn check_account_without_password_reports_password_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = Keep2ShareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("keep2share.password_missing"));
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[path = "check_tests.rs"]
mod check_tests;

#[path = "free_tests.rs"]
mod free_tests;

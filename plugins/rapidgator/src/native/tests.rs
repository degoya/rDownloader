use std::sync::Arc;

use rd_plugin_api::test_support::ScriptedHost as MockHost;
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};
use rd_plugin_types::{AccountId, FailureKind, LinkStatus};
use url::Url;

use super::RapidgatorResolver;

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
    json_response(200, "https://rapidgator.net/api/v2/user/login", body)
}

fn file_info_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://rapidgator.net/api/v2/file/info", body)
}

fn file_download_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://rapidgator.net/api/v2/file/download", body)
}

const FILE_ID: &str = "123456";
const FILE_URL: &str = "https://rapidgator.net/file/123456/release.html";

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

fn query_value<'a>(request: &'a HostHttpRequest, name: &str) -> Option<&'a str> {
    request
        .query
        .iter()
        .find(|value| value.name == name)
        .map(|value| value.value_template.as_str())
}

const LOGIN_OK: &str = r#"{"response":{"token":"tok-abc","user":{"is_premium":true,"premium_end_time":1798761600,"traffic":{"left":2048,"total":4096}}},"status":200,"details":null}"#;
const LOGIN_OK_FREE: &str =
    r#"{"response":{"token":"tok-abc","user":{"is_premium":false}},"status":200,"details":null}"#;

#[tokio::test]
async fn happy_path_resolve_logs_in_then_gets_file_info_then_download_url() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            file_info_response(
                r#"{"response":{"file":{"name":"release.rar","size":4096,"hash":"deadbeef"}},"status":200,"details":null}"#,
            ),
            file_download_response(
                r#"{"response":{"download_url":"https://pr1.rapidgator.net/d/tok-abc/release.rar"},"status":200,"details":null}"#,
            ),
        ],
        true,
    );
    let resolver = RapidgatorResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://pr1.rapidgator.net/d/tok-abc/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));
    assert_eq!(resolved.checksum, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 3);

    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://rapidgator.net/api/v2/user/login"
    );
    assert_eq!(query_value(&requests[0], "login"), Some("{{username}}"));
    assert_eq!(
        query_value(&requests[0], "password"),
        Some("{{secret:rapidgator_password}}")
    );
    assert_eq!(
        requests[0]
            .query
            .iter()
            .map(|value| value.name.as_str())
            .collect::<Vec<_>>(),
        vec!["login", "password"]
    );

    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].url.as_str(),
        "https://rapidgator.net/api/v2/file/info"
    );
    assert_eq!(query_value(&requests[1], "token"), Some("tok-abc"));
    assert_eq!(query_value(&requests[1], "file_id"), Some(FILE_ID));

    assert_eq!(requests[2].method, "GET");
    assert_eq!(
        requests[2].url.as_str(),
        "https://rapidgator.net/api/v2/file/download"
    );
    assert_eq!(query_value(&requests[2], "token"), Some("tok-abc"));
    assert_eq!(query_value(&requests[2], "file_id"), Some(FILE_ID));
    assert!(requests[2].body.is_empty());
}

#[tokio::test]
async fn resolve_without_password_reports_password_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = RapidgatorResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("rapidgator.password_missing"));
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn resolve_with_wrong_password_reports_account_invalid() {
    let host = MockHost::new(
        login_response(r#"{"response":null,"status":401,"details":"Login or password is wrong"}"#),
        true,
    );
    let resolver = RapidgatorResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("wrong password");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("rapidgator.bad_credentials"));
}

#[tokio::test]
async fn resolve_offline_file_reports_offline() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            file_info_response(r#"{"response":null,"status":404,"details":"Not found"}"#),
        ],
        true,
    );
    let resolver = RapidgatorResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("rapidgator.file_offline"));
}

#[tokio::test]
async fn resolve_untrusted_404_on_file_download_retries_instead_of_offline() {
    // Exactly JD's documented bug scenario: `file/info` just confirmed the file online, but
    // `file/download` spuriously answers 404 — JD does not trust a 404 on this specific endpoint
    // (`trustError404=false`) and retries (60s) rather than declaring the file offline.
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            file_info_response(
                r#"{"response":{"file":{"name":"release.rar","size":4096}},"status":200,"details":null}"#,
            ),
            file_download_response(r#"{"response":null,"status":404,"details":"Not found"}"#),
        ],
        true,
    );
    let resolver = RapidgatorResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("untrusted 404");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(60)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("rapidgator.download_link_unconfirmed")
    );
}

#[tokio::test]
async fn resolve_traffic_limit_reports_rate_limited_with_300s_retry() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            file_info_response(
                r#"{"response":{"file":{"name":"release.rar","size":4096}},"status":200,"details":null}"#,
            ),
            file_download_response(
                r#"{"response":null,"status":423,"details":"Error: Exceeded traffic"}"#,
            ),
        ],
        true,
    );
    let resolver = RapidgatorResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("traffic limit");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("rapidgator.limit_reached"));
}

#[tokio::test]
async fn resolve_login_throttled_reports_rate_limited_with_300s_retry() {
    let host = MockHost::new(
        login_response(r#"{"response":null,"status":400,"details":"Please wait before retrying"}"#),
        true,
    );
    let resolver = RapidgatorResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("login throttled");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("rapidgator.login_throttled"));
    // Login is the only request sent; throttling is detected before any further call.
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[tokio::test]
async fn check_account_reports_premium_traffic_and_expiry_label() {
    let host = MockHost::new(login_response(LOGIN_OK), true);
    let resolver = RapidgatorResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
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
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.as_str(),
        "https://rapidgator.net/api/v2/user/login"
    );
}

#[tokio::test]
async fn check_account_free_account_is_not_premium() {
    let host = MockHost::new(login_response(LOGIN_OK_FREE), true);
    let resolver = RapidgatorResolver::new(host as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(!status.premium);
    assert_eq!(plugin_common::native::label_summary(&status.label), "");
    assert_eq!(status.traffic_left, None);
}

#[tokio::test]
async fn check_account_without_password_reports_password_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = RapidgatorResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("rapidgator.password_missing"));
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn check_maps_online_offline_and_unparseable_batch() {
    let host = MockHost::with_responses(
        vec![
            login_response(LOGIN_OK),
            file_info_response(
                r#"{"response":{"file":{"name":"a.rar","size":10}},"status":200,"details":null}"#,
            ),
            file_info_response(r#"{"response":null,"status":404,"details":"Not found"}"#),
        ],
        true,
    );
    let resolver = RapidgatorResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                "https://rapidgator.net/file/123456"
                    .parse::<Url>()
                    .expect("URL"),
                "https://rapidgator.net/file/654321"
                    .parse::<Url>()
                    .expect("URL"),
                "https://rapidgator.net/article/premium"
                    .parse::<Url>()
                    .expect("URL"),
            ],
            client: client_identity(),
        })
        .await
        .expect("results");

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("a.rar"));
    assert_eq!(results[0].size.map(|size| size.get()), Some(10));
    assert_eq!(results[1].status, LinkStatus::Offline);
    // Not a `/file/<id>` link -> never reaches any HTTP call.
    assert_eq!(results[2].status, LinkStatus::Unknown);

    let requests = host.requests.lock().expect("mock lock");
    // One login + one file/info per checkable link (no batching, JD has none for this endpoint).
    assert_eq!(requests.len(), 3);
    assert_eq!(query_value(&requests[1], "file_id"), Some("123456"));
    assert_eq!(query_value(&requests[2], "file_id"), Some("654321"));
}

#[path = "free_tests.rs"]
mod free_tests;

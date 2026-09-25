use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver,
    ResolverHost,
};

use super::DebridLinkResolver;

struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    has_secret: bool,
}

impl MockHost {
    fn new(response: HostHttpResponse, has_secret: bool) -> Arc<Self> {
        Self::with_responses(vec![response], has_secret)
    }

    fn with_responses(responses: Vec<HostHttpResponse>, has_secret: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret,
        })
    }
}

#[async_trait]
impl ResolverHost for MockHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests.lock().expect("mock lock").push(request);
        self.responses
            .lock()
            .expect("mock lock")
            .pop_front()
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "missing mock response"))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        self.has_secret
    }
}

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

fn add_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://debrid-link.com/api/v2/downloader/add", body)
}

fn account_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://debrid-link.com/api/v2/account/infos", body)
}

fn hosts_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://debrid-link.com/api/v2/downloader/hosts", body)
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://example.test/f/abc123".parse().expect("URL"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

fn assert_bearer_and_form_headers(request: &HostHttpRequest) {
    assert!(request.headers.iter().any(|header| {
        header.name == "Authorization"
            && header.value_template == "Bearer {{secret:debridlink_api_key}}"
    }));
    assert!(request.headers.iter().any(|header| {
        header.name == "Content-Type"
            && header.value_template == "application/x-www-form-urlencoded"
    }));
}

#[tokio::test]
async fn happy_path_resolve_posts_downloader_add() {
    let host = MockHost::new(
        add_response(
            r#"{"success":true,"value":{"downloadUrl":"https://cache.debrid-link.com/dl/tok/release.rar","name":"release.rar","size":4096,"chunk":16}}"#,
        ),
        true,
    );
    let resolver = DebridLinkResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://cache.debrid-link.com/dl/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));
    assert_eq!(resolved.checksum, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].url.as_str(),
        "https://debrid-link.com/api/v2/downloader/add"
    );
    assert_eq!(
        requests[0].body,
        b"url=https%3A%2F%2Fexample.test%2Ff%2Fabc123"
    );
    assert_bearer_and_form_headers(&requests[0]);
}

#[tokio::test]
async fn resolve_without_api_key_reports_api_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = DebridLinkResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("debridlink.api_key_missing"));
    assert!(
        host.requests.lock().expect("mock lock").is_empty(),
        "no HTTP request should be issued without a configured API key"
    );
}

#[tokio::test]
async fn resolve_with_bad_token_reports_account_invalid() {
    let host = MockHost::new(
        add_response(r#"{"success":false,"error":"badToken"}"#),
        true,
    );
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("bad token");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("debridlink.auth_invalid"));
    assert_eq!(
        failure.params.get("api_code").map(String::as_str),
        Some("badToken")
    );
}

#[tokio::test]
async fn resolve_file_not_found_reports_offline() {
    let host = MockHost::new(
        add_response(r#"{"success":false,"error":"fileNotFound"}"#),
        true,
    );
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("debridlink.file_offline"));
}

#[tokio::test]
async fn resolve_max_link_reports_rate_limited_3600() {
    let host = MockHost::new(add_response(r#"{"success":false,"error":"maxLink"}"#), true);
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("limit reached");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("debridlink.limit_reached"));
}

#[tokio::test]
async fn resolve_flood_detected_reports_rate_limited_3600() {
    let host = MockHost::new(
        add_response(r#"{"success":false,"error":"floodDetected"}"#),
        true,
    );
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("flood");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("debridlink.flood"));
}

#[tokio::test]
async fn resolve_host_not_valid_reports_transient_300() {
    let host = MockHost::new(
        add_response(r#"{"success":false,"error":"hostNotValid"}"#),
        true,
    );
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("host unsupported");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("debridlink.host_unsupported"));
}

#[tokio::test]
async fn resolve_without_download_url_reports_no_download_url() {
    let host = MockHost::new(
        add_response(r#"{"success":true,"value":{"chunk":1}}"#),
        true,
    );
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("no download url");
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("debridlink.no_download_url"));
}

#[tokio::test]
async fn check_account_reports_premium_and_label() {
    let host = MockHost::new(
        account_response(
            r#"{"success":true,"value":{"accountType":1,"premiumLeft":86400,"email":"a@test.example","pseudo":"alice"}}"#,
        ),
        true,
    );
    let resolver = DebridLinkResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=alice)"
    );
    assert_eq!(status.traffic_left, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://debrid-link.com/api/v2/account/infos"
    );
    assert_bearer_and_form_headers(&requests[0]);
}

#[tokio::test]
async fn check_account_free_is_not_premium_and_falls_back_to_email() {
    let host = MockHost::new(
        account_response(
            r#"{"success":true,"value":{"accountType":0,"email":"bob@test.example"}}"#,
        ),
        true,
    );
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(!status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=bob@test.example)"
    );
}

#[tokio::test]
async fn check_account_without_api_key_reports_api_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = DebridLinkResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("debridlink.api_key_missing"));
    assert!(
        host.requests.lock().expect("mock lock").is_empty(),
        "no HTTP request should be issued without a configured API key"
    );
}

#[tokio::test]
async fn hosters_flattens_and_dedupes_from_fixture() {
    let host = MockHost::new(
        hosts_response(
            r#"{"success":true,"value":[
                {"name":"1fichier","status":1,"isFree":true,"domains":["1fichier.com"]},
                {"name":"rapidgator","status":1,"isFree":false,"domains":["rapidgator.net","RAPIDGATOR.NET"]},
                {"name":"youtube","status":1,"isFree":true,"domains":["youtube.com"]}
            ]}"#,
        ),
        true,
    );
    let resolver = DebridLinkResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let hosters = resolver.hosters(AccountId::new()).await.expect("hosters");
    assert_eq!(
        hosters,
        vec!["1fichier.com", "rapidgator.net", "youtube.com"]
    );

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://debrid-link.com/api/v2/downloader/hosts?keys=status,isFree,name,domains"
    );
}

#[tokio::test]
async fn hosters_without_api_key_reports_api_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = DebridLinkResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .hosters(AccountId::new())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("debridlink.api_key_missing"));
    assert!(
        host.requests.lock().expect("mock lock").is_empty(),
        "no HTTP request should be issued without a configured API key"
    );
}

#[tokio::test]
async fn check_defaults_to_unsupported() {
    // No batch link-status endpoint exists on Debrid-Link's API per JD's `DebridLinkCom.java`
    // (`requestFileInformation` returns `AvailableStatus.UNCHECKABLE`) — `check()` is
    // intentionally left as the `Resolver` trait's default (`rd_plugin_api::Resolver::check`),
    // which reports `link.check_unsupported`. `guest.rs` cannot rely on that default (the WIT
    // `Guest` trait has none) and instead reproduces it via `messages::CHECK_UNSUPPORTED`;
    // asserting the trait default's actual output against that same constant here means a future
    // change to the trait default's text would fail this test instead of silently desyncing
    // native and guest.
    let host = MockHost::with_responses(Vec::new(), true);
    let resolver = DebridLinkResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .check(rd_plugin_api::CheckRequest {
            urls: vec!["https://example.test/f/abc".parse().expect("URL")],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect_err("unsupported");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(
        failure.code.as_deref(),
        Some(crate::messages::CHECK_UNSUPPORTED.0)
    );
    assert_eq!(failure.message, crate::messages::CHECK_UNSUPPORTED.1);
}

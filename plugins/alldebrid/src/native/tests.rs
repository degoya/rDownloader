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

use super::AllDebridResolver;

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

fn unlock_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://api.alldebrid.com/v4.1/link/unlock", body)
}

fn delayed_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://api.alldebrid.com/v4.1/link/delayed", body)
}

fn user_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://api.alldebrid.com/v4.1/user", body)
}

fn hosts_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://api.alldebrid.com/v4.1/user/hosts", body)
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
            && header.value_template == "Bearer {{secret:alldebrid_api_key}}"
    }));
    assert!(request.headers.iter().any(|header| {
        header.name == "Content-Type"
            && header.value_template == "application/x-www-form-urlencoded"
    }));
}

#[tokio::test]
async fn happy_path_resolve_posts_link_unlock() {
    let host = MockHost::new(
        unlock_response(
            r#"{"status":"success","data":{"link":"https://cdn.alldebrid.com/dl/tok/release.rar","filename":"release.rar","filesize":4096}}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://cdn.alldebrid.com/dl/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));
    assert_eq!(resolved.checksum, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].url.as_str(),
        "https://api.alldebrid.com/v4.1/link/unlock"
    );
    assert_eq!(
        requests[0].body,
        b"link=https%3A%2F%2Fexample.test%2Ff%2Fabc123"
    );
    assert_bearer_and_form_headers(&requests[0]);
}

#[tokio::test]
async fn resolve_without_api_key_reports_api_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("alldebrid.api_key_missing"));
}

#[tokio::test]
async fn resolve_with_bad_api_key_reports_account_invalid() {
    let host = MockHost::new(
        unlock_response(
            r#"{"status":"error","error":{"code":"AUTH_BAD_APIKEY","message":"The auth apikey is invalid"}}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("bad key");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("alldebrid.auth_invalid"));
    assert_eq!(
        failure.params.get("api_code").map(String::as_str),
        Some("AUTH_BAD_APIKEY")
    );
}

#[tokio::test]
async fn resolve_link_down_reports_offline() {
    let host = MockHost::new(
        unlock_response(
            r#"{"status":"error","error":{"code":"LINK_DOWN","message":"File not found"}}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("alldebrid.link_down"));
}

#[tokio::test]
async fn resolve_unsupported_host_reports_unsupported() {
    let host = MockHost::new(
        unlock_response(
            r#"{"status":"error","error":{"code":"LINK_HOST_NOT_SUPPORTED","message":"unsupported host"}}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("unsupported");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("alldebrid.host_unsupported"));
}

#[tokio::test]
async fn resolve_delayed_still_processing_reports_transient_with_retry_after_10() {
    let host = MockHost::with_responses(
        vec![
            unlock_response(
                r#"{"status":"success","data":{"link":"https://cdn.alldebrid.com/dl/tok/release.rar","filename":"release.rar","filesize":4096,"delayed":42}}"#,
            ),
            delayed_response(r#"{"status":"success","data":{"status":1,"progress":0.5}}"#),
        ],
        true,
    );
    let resolver = AllDebridResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("still processing");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(10)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("alldebrid.link_delayed"));

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].url.as_str(),
        "https://api.alldebrid.com/v4.1/link/delayed"
    );
    assert_eq!(requests[1].body, b"id=42");
}

#[tokio::test]
async fn resolve_delayed_available_completes_with_the_unlock_link() {
    let host = MockHost::with_responses(
        vec![
            unlock_response(
                r#"{"status":"success","data":{"link":"https://cdn.alldebrid.com/dl/tok/release.rar","filename":"release.rar","filesize":4096,"delayed":42}}"#,
            ),
            delayed_response(r#"{"status":"success","data":{"status":2,"progress":1}}"#),
        ],
        true,
    );
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://cdn.alldebrid.com/dl/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
}

#[tokio::test]
async fn resolve_delayed_error_reports_transient_with_retry_after_300() {
    let host = MockHost::with_responses(
        vec![
            unlock_response(
                r#"{"status":"success","data":{"link":"https://cdn.alldebrid.com/dl/tok/release.rar","delayed":42}}"#,
            ),
            delayed_response(r#"{"status":"success","data":{"status":3,"progress":0}}"#),
        ],
        true,
    );
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("delayed error");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("alldebrid.temporarily_unavailable")
    );
}

#[tokio::test]
async fn resolve_maintenance_reports_transient_with_retry_after_300() {
    let host = MockHost::new(
        unlock_response(
            r#"{"status":"error","error":{"code":"MAINTENANCE","message":"Under maintenance"}}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("maintenance");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("alldebrid.temporarily_unavailable")
    );
}

#[tokio::test]
async fn check_account_reports_premium_and_label() {
    let host = MockHost::new(
        user_response(
            r#"{"status":"success","data":{"user":{"username":"alice","isPremium":true,"premiumUntil":1893456000}}}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
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
        "https://api.alldebrid.com/v4.1/user"
    );
    assert_bearer_and_form_headers(&requests[0]);
}

#[tokio::test]
async fn check_account_free_is_not_premium() {
    let host = MockHost::new(
        user_response(
            r#"{"status":"success","data":{"user":{"username":"bob","isPremium":false}}}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(!status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=bob)"
    );
}

#[tokio::test]
async fn check_account_without_api_key_reports_api_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("alldebrid.api_key_missing"));
}

#[tokio::test]
async fn hosters_flattens_and_dedupes_from_fixture() {
    let host = MockHost::new(
        hosts_response(
            r#"{"status":"success","data":{
                "hosts": {
                    "1fichier": {"name": "1fichier", "domains": ["1fichier.com"]},
                    "rapidgator": {"name": "rapidgator", "domains": ["rapidgator.net", "RAPIDGATOR.NET"]}
                },
                "streams": {
                    "youtube": {"name": "youtube", "domains": ["youtube.com"]}
                }
            }}"#,
        ),
        true,
    );
    let resolver = AllDebridResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let hosters = resolver.hosters(AccountId::new()).await.expect("hosters");
    assert_eq!(
        hosters,
        vec!["1fichier.com", "rapidgator.net", "youtube.com"]
    );

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://api.alldebrid.com/v4.1/user/hosts"
    );
}

#[tokio::test]
async fn hosters_without_api_key_reports_api_key_missing() {
    // Finding 1 (review fix): `/user/hosts` is an authenticated endpoint like `/user` and
    // `/link/unlock`; `hosters()` must fail fast on the same secret gate instead of issuing the
    // HTTP request regardless.
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = AllDebridResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .hosters(AccountId::new())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("alldebrid.api_key_missing"));
    assert!(
        host.requests.lock().expect("mock lock").is_empty(),
        "no HTTP request should be issued without a configured API key"
    );
}

#[tokio::test]
async fn check_defaults_to_unsupported() {
    // No `/link/infos` batch endpoint exists on AllDebrid's API per JD's `AllDebridCom.java`
    // (`requestFileInformation` re-runs `link/unlock` instead) — `check()` is intentionally left
    // as the `Resolver` trait's default (`rd_plugin_api::Resolver::check`), which reports
    // `link.check_unsupported`. `guest.rs` cannot rely on that default (the WIT `Guest` trait has
    // none) and instead reproduces it via `messages::CHECK_UNSUPPORTED`; asserting the trait
    // default's actual output against that same constant here (finding 3, review fix) means a
    // future change to the trait default's text would fail this test instead of silently
    // desyncing native and guest.
    let host = MockHost::with_responses(Vec::new(), true);
    let resolver = AllDebridResolver::new(host as Arc<dyn ResolverHost>);
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

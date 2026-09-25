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

use super::LinkSnappyResolver;

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

fn linkgen_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://linksnappy.com/api/linkgen", body)
}

fn userdetails_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://linksnappy.com/api/USERDETAILS", body)
}

fn filehosts_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://linksnappy.com/api/FILEHOSTS", body)
}

const RESOLVE_URL: &str = "https://rapidgator.net/file/123456/release.rar";
const EXPECTED_GEN_LINKS: &str = r#"{"link":"https://rapidgator.net/file/123456/release.rar","type":"","username":"{{username}}","password":"{{secret:linksnappy_password}}"}"#;

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: RESOLVE_URL.parse().expect("URL"),
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

#[tokio::test]
async fn happy_path_resolve_sends_gen_links_with_templates_and_url_encoding() {
    let host = MockHost::new(
        linkgen_response(
            r#"{"status":"OK","links":[{"status":"OK","generated":"https://cdn.linksnappy.com/dl/tok/release.rar","filename":"release.rar"}]}"#,
        ),
        true,
    );
    let resolver = LinkSnappyResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://cdn.linksnappy.com/dl/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size, None);
    assert_eq!(resolved.checksum, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://linksnappy.com/api/linkgen"
    );
    assert_eq!(requests[0].query.len(), 1);
    assert_eq!(requests[0].query[0].name, "genLinks");
    // Exact literal template text — the host expands `{{username}}`/`{{secret:...}}` (and
    // URL-encodes the whole query value) after this request leaves the plugin.
    assert_eq!(requests[0].query[0].value_template, EXPECTED_GEN_LINKS);
    assert!(requests[0].headers.is_empty());
    assert!(requests[0].body.is_empty());
}

#[tokio::test]
async fn resolve_without_password_reports_password_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = LinkSnappyResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("linksnappy.password_missing"));
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn resolve_with_bad_credentials_reports_account_invalid() {
    let host = MockHost::new(
        linkgen_response(r#"{"status":"ERROR","error":"Incorrect Username or Password"}"#),
        true,
    );
    let resolver = LinkSnappyResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("bad credentials");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("linksnappy.bad_credentials"));
}

#[tokio::test]
async fn resolve_file_not_found_reports_offline() {
    let host = MockHost::new(
        linkgen_response(
            r#"{"status":"OK","links":[{"status":"ERROR","error":"File not found"}]}"#,
        ),
        true,
    );
    let resolver = LinkSnappyResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("linksnappy.file_offline"));
}

#[tokio::test]
async fn resolve_daily_limit_reports_rate_limited_with_300s_retry() {
    let host = MockHost::new(
        linkgen_response(r#"{"status":"ERROR","error":"Account has exceeded the daily quota"}"#),
        true,
    );
    let resolver = LinkSnappyResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("daily limit");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("linksnappy.limit_reached"));
}

#[tokio::test]
async fn resolve_per_link_limit_reports_rate_limited_with_60s_retry() {
    let host = MockHost::new(
        linkgen_response(
            r#"{"status":"OK","links":[{"status":"ERROR","error":"You have reached max download limit of 10GB"}]}"#,
        ),
        true,
    );
    let resolver = LinkSnappyResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("per-link limit");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(60)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("linksnappy.limit_reached"));
}

#[tokio::test]
async fn resolve_without_links_array_reports_no_download_url_as_transient() {
    let host = MockHost::new(linkgen_response(r#"{"status":"OK"}"#), true);
    let resolver = LinkSnappyResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("no links");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: None
        }
    );
    assert_eq!(failure.code.as_deref(), Some("linksnappy.no_download_url"));
}

#[tokio::test]
async fn check_account_reports_premium_and_traffic_left() {
    let host = MockHost::new(
        userdetails_response(
            r#"{"status":"OK","return":{"expire":1999999999,"usedspace":10,"trafficused":5,"trafficleft":123456,"maxtraffic":999999}}"#,
        ),
        true,
    );
    let resolver = LinkSnappyResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(plugin_common::native::label_summary(&status.label), "");
    assert_eq!(status.traffic_left.map(|bytes| bytes.get()), Some(123456));

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://linksnappy.com/api/USERDETAILS"
    );
    assert_eq!(query_value(&requests[0], "username"), Some("{{username}}"));
    assert_eq!(
        query_value(&requests[0], "password"),
        Some("{{secret:linksnappy_password}}")
    );
}

#[tokio::test]
async fn check_account_expired_is_not_premium() {
    let host = MockHost::new(
        userdetails_response(
            r#"{"status":"OK","return":{"expire":"expired","trafficleft":"unlimited"}}"#,
        ),
        true,
    );
    let resolver = LinkSnappyResolver::new(host as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(!status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.premium_expired()"
    );
    assert_eq!(status.traffic_left, None);
}

#[tokio::test]
async fn check_account_without_password_reports_password_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = LinkSnappyResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("linksnappy.password_missing"));
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn hosters_flattens_domains_and_aliases_lowercase_dedupe_and_sort() {
    let host = MockHost::new(
        filehosts_response(
            r#"{"status":"OK","return":{"Rapidgator.net":{"alias":["RG.TO","rapidgator.asia"]},"1fichier.com":{"alias":["1fichier.com"," "]}}}"#,
        ),
        true,
    );
    let resolver = LinkSnappyResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let hosters = resolver.hosters(AccountId::new()).await.expect("hosters");
    assert_eq!(
        hosters,
        vec![
            "1fichier.com".to_owned(),
            "rapidgator.asia".to_owned(),
            "rapidgator.net".to_owned(),
            "rg.to".to_owned(),
        ]
    );

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://linksnappy.com/api/FILEHOSTS"
    );
    // FILEHOSTS is authenticated (JD never calls it without a prior login) — same credential
    // pair as USERDETAILS.
    assert_eq!(query_value(&requests[0], "username"), Some("{{username}}"));
    assert_eq!(
        query_value(&requests[0], "password"),
        Some("{{secret:linksnappy_password}}")
    );
}

#[tokio::test]
async fn hosters_without_password_reports_password_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = LinkSnappyResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .hosters(AccountId::new())
        .await
        .expect_err("missing password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("linksnappy.password_missing"));
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn check_defaults_to_unsupported() {
    // LinkSnappy has no batch link-status endpoint (see `messages::CHECK_UNSUPPORTED`'s doc
    // comment) — `check()` is intentionally left as the `Resolver` trait's default
    // (`rd_plugin_api::Resolver::check`), which reports `link.check_unsupported`. `guest.rs`
    // cannot rely on that default (the WIT `Guest` trait has none) and instead reproduces it via
    // `messages::CHECK_UNSUPPORTED`; asserting the trait default's actual output against that
    // same constant here means a future change to the trait default's text would fail this test
    // instead of silently desyncing native and guest.
    let host = MockHost::with_responses(Vec::new(), true);
    let resolver = LinkSnappyResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .check(rd_plugin_api::CheckRequest {
            urls: vec!["https://example.test/f/abc".parse().expect("URL")],
            client: client_identity(),
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

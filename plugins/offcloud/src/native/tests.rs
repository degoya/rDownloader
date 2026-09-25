//! The resolver, driven against a mock of `offcloud.com/api`.
//!
//! No socket is opened, no account is needed and no request leaves the machine: the mock
//! answers at the host boundary, so a test also sees each request exactly as the plugin
//! described it — which is what lets it assert that the account's key left the plugin as the
//! template `{{secret:offcloud_api_key}}` and never as a value.
//!
//! **A run against the real provider is not claimed here.** It needs an Offcloud account;
//! `docs/roadmap/jobs/120-02-offcloud.md` records that as open.

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

use super::OffcloudResolver;

const KEY_TEMPLATE: &str = "Bearer {{secret:offcloud_api_key}}";

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

    fn requests(&self) -> Vec<HostHttpRequest> {
        self.requests.lock().expect("mock lock").clone()
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

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_secret && reference == "offcloud_api_key"
    }
}

fn answer(status: u16, path: &str, body: &str, retry_after: Option<&str>) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: format!("https://offcloud.com/api{path}")
            .parse()
            .expect("URL"),
        headers: retry_after
            .map(|value| ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: value.to_owned(),
            })
            .into_iter()
            .collect(),
        body: body.as_bytes().to_vec(),
    }
}

fn instant(body: &str) -> HostHttpResponse {
    answer(200, "/instant", body, None)
}

fn account(body: &str) -> HostHttpResponse {
    answer(200, "/account/info", body, None)
}

fn resolver(host: &Arc<MockHost>) -> OffcloudResolver {
    OffcloudResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://example.invalid/f/abc123".parse().expect("URL"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

fn assert_bearer_template(request: &HostHttpRequest) {
    let authorization = request
        .headers
        .iter()
        .find(|header| header.name == "Authorization")
        .expect("an Authorization header");
    assert_eq!(authorization.value_template, KEY_TEMPLATE);
}

#[tokio::test]
async fn a_link_is_resolved_through_the_instant_endpoint() {
    let host = MockHost::new(
        instant(
            r#"{"requestId":"REDACTEDREQUEST01","fileName":"release.rar","size":4096,"url":"https://s1.offcloud.com/instant/REDACTEDREQUEST01/release.rar","site":"example","status":"created"}"#,
        ),
        true,
    );
    let resolved = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://s1.offcloud.com/instant/REDACTEDREQUEST01/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));

    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].url.as_str(), "https://offcloud.com/api/instant");
    assert_eq!(
        requests[0].body,
        b"url=https%3A%2F%2Fexample.invalid%2Ff%2Fabc123"
    );
    assert_bearer_template(&requests[0]);
    // The key travels in a header and never in the address: a query parameter would end up in
    // every redirect chain and every log line that quotes a URL.
    assert!(requests[0].query.is_empty());
}

#[tokio::test]
async fn every_resolve_asks_again_rather_than_reusing_an_address() {
    // How a short-lived Offcloud address is renewed: there is no cache to go stale, so the
    // second call is a second request and comes back with the newer address.
    let host = MockHost::with_responses(
        vec![
            instant(r#"{"url":"https://s1.offcloud.com/instant/REDACTED01/a.bin"}"#),
            instant(r#"{"url":"https://s1.offcloud.com/instant/REDACTED02/a.bin"}"#),
        ],
        true,
    );
    let resolver = resolver(&host);
    let first = resolver.resolve(resolve_request()).await.expect("resolved");
    let second = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_ne!(first.url, second.url);
    assert_eq!(host.requests().len(), 2);
}

#[tokio::test]
async fn resolving_without_an_api_key_reaches_nothing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("offcloud.api_key_missing"));
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn an_expired_key_is_an_invalid_account_and_not_a_wait() {
    // Offcloud answers this with a 200 and a document, which is why the document is read
    // before the status.
    let host = MockHost::new(instant(r#"{"error":"NOAUTH"}"#), true);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("offcloud.auth_invalid"));
}

#[tokio::test]
async fn a_missing_addon_is_unsupported_and_names_which_one() {
    let host = MockHost::new(instant(r#"{"not_available":"premium"}"#), true);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("offcloud.addon_required"));
    assert_eq!(
        failure.params.get("addon").map(String::as_str),
        Some("premium")
    );
}

#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_retry_after() {
    let host = MockHost::new(answer(429, "/instant", "", Some("120")), true);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(120)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("offcloud.rate_limited"));
}

#[tokio::test]
async fn an_answer_without_an_address_and_one_with_an_unusable_one_are_both_refused() {
    let host = MockHost::new(instant(r#"{"requestId":"REDACTEDREQUEST01"}"#), true);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("offcloud.no_download_url"));

    // The address comes back from the provider and goes out again as a request, so a scheme
    // the queue never agreed to is refused here rather than somewhere further in.
    let host = MockHost::new(instant(r#"{"url":"file:///etc/passwd"}"#), true);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("offcloud.bad_download_url"));
}

#[tokio::test]
async fn a_premium_account_is_usable_and_says_until_when() {
    let host = MockHost::new(
        account(
            r#"{"userId":"REDACTEDUSER01","isPremium":true,"canDownload":true,"expirationDate":"2027-01-31"}"#,
        ),
        true,
    );
    let status = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=REDACTEDUSER01) plugin.account.premium_until(until=2027-01-31)"
    );

    let requests = host.requests();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://offcloud.com/api/account/info"
    );
    assert_bearer_template(&requests[0]);
}

#[tokio::test]
async fn a_free_account_and_a_blocked_one_are_told_apart() {
    let host = MockHost::new(account(r#"{"isPremium":false,"canDownload":true}"#), true);
    let status = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid, "the key answered, so it is a key");
    assert!(!status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.premium_expired()"
    );

    // Premium and refused anyway. Telling this person to buy the add-on they already own is
    // the one answer that cannot help, so it has a code of its own.
    let host = MockHost::new(account(r#"{"isPremium":true,"canDownload":false}"#), true);
    let status = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(!status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "offcloud.download_blocked()"
    );
}

#[tokio::test]
async fn the_hoster_catalogue_comes_from_the_sites_endpoint() {
    let host = MockHost::new(
        answer(
            200,
            "/sites",
            r#"[{"name":"Rapidgator","hosts":["rapidgator.net","RAPIDGATOR.NET"]},
                {"name":"1fichier","domains":["www.1fichier.com"]},
                {"name":"Some Video Site"}]"#,
            None,
        ),
        true,
    );
    let hosters = resolver(&host)
        .hosters(AccountId::new())
        .await
        .expect("hosters");
    assert_eq!(hosters, vec!["1fichier.com", "rapidgator.net"]);
    assert_eq!(
        host.requests()[0].url.as_str(),
        "https://offcloud.com/api/sites"
    );
}

#[tokio::test]
async fn checking_links_is_refused_rather_than_started() {
    // `/api/instant` would answer the question by spending an allowance on a download nobody
    // asked for, and `/api/cache` answers about BitTorrent content only.
    let host = MockHost::with_responses(Vec::new(), true);
    let failure = resolver(&host)
        .check(rd_plugin_api::CheckRequest {
            urls: vec!["https://example.invalid/f/abc".parse().expect("URL")],
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
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn the_plugin_serves_the_provider_row_its_sibling_claims() {
    // `plugins/offcloud-cloud/` carries `claims = ["offcloud"]` and no `[provider]` of its
    // own, so this manifest is the one that has to name the slug. If the two ever disagree,
    // the remote-job plugin has an account nobody can create.
    let host = MockHost::with_responses(Vec::new(), true);
    let metadata = resolver(&host).metadata().clone();
    assert_eq!(metadata.provider_slug, "offcloud");
    assert!(metadata.requires_account, "a multihoster has no free path");
    assert_eq!(metadata.max_concurrent_downloads, 3);
}

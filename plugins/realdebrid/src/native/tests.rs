//! The Real-Debrid contract, driven against a mock of the provider's API.
//!
//! The mock answers at the host boundary rather than over a socket, so no request leaves the
//! machine and no account is needed. What it proves is what a host reacts to differently:
//!
//! | Case | What the provider answered | What the plugin must do |
//! | --- | --- | --- |
//! | Success | `200` with a `download` address | queue that address, with name and size |
//! | Rate limit | `429` + `Retry-After` / `error_code` 34 | wait the stated time, keep the account |
//! | Sign-in expired | `error_code` 8 | invalidate the account so a sign-in is offered |
//! | Error | `error_code` 24 / an undocumented number | offline, or a generic coded failure |
//!
//! A run against the real provider is deliberately **not** claimed here: it needs an account,
//! and the job file records which acceptance criteria that leaves unproven.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind, LinkStatus};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};

use super::RealDebridResolver;

struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    has_token: bool,
}

impl MockHost {
    fn new(response: HostHttpResponse, has_token: bool) -> Arc<Self> {
        Self::with_responses(vec![response], has_token)
    }

    fn with_responses(responses: Vec<HostHttpResponse>, has_token: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_token,
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

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        self.has_token
    }
}

fn answer(status: u16, path: &str, body: &str, headers: Vec<ResolvedHeader>) -> HostHttpResponse {
    let mut all = vec![ResolvedHeader {
        name: "Content-Type".to_owned(),
        value: "application/json".to_owned(),
    }];
    all.extend(headers);
    HostHttpResponse {
        status,
        final_url: format!("https://api.real-debrid.com/rest/1.0{path}")
            .parse()
            .expect("URL"),
        headers: all,
        body: body.as_bytes().to_vec(),
    }
}

fn unrestrict(status: u16, body: &str) -> HostHttpResponse {
    answer(status, "/unrestrict/link", body, Vec::new())
}

fn resolver(host: &Arc<MockHost>) -> RealDebridResolver {
    RealDebridResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

fn client() -> ClientIdentity {
    ClientIdentity {
        account_id: Some(AccountId::new()),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://example.test/f/abc123".parse().expect("URL"),
        client: client(),
    }
}

/// The token leaves the plugin as a template and never as a value. Asserted on the request the
/// host was handed, which is the only place a leak could be seen from the outside.
fn assert_bearer_template(request: &HostHttpRequest) {
    assert!(
        request.headers.iter().any(|header| {
            header.name == "Authorization"
                && header.value_template == "Bearer {{secret:realdebrid_access_token}}"
        }),
        "the Bearer header must carry the template, not a token"
    );
}

#[tokio::test]
async fn a_successful_unrestriction_queues_the_generated_address() {
    let host = MockHost::new(
        unrestrict(
            200,
            r#"{"id":"XYZ","filename":"release.rar","filesize":4096,
                "link":"https://example.test/f/abc123","host":"example.test","chunks":16,
                "download":"https://34.download.real-debrid.com/d/TOKEN/release.rar"}"#,
        ),
        true,
    );
    let resolved = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect("resolved");

    // `download`, not the `link` that went in: queueing the second would fetch the hoster page.
    assert_eq!(
        resolved.url.as_str(),
        "https://34.download.real-debrid.com/d/TOKEN/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));
    // Real-Debrid's `crc` is a flag, not a digest, so nothing must arrive as one.
    assert_eq!(resolved.checksum, None);

    let requests = host.requests();
    let request = requests.first().expect("one request");
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.url.as_str(),
        "https://api.real-debrid.com/rest/1.0/unrestrict/link"
    );
    assert_eq!(
        request.body,
        b"link=https%3A%2F%2Fexample.test%2Ff%2Fabc123"
    );
    assert_bearer_template(request);
}

/// A rate limit is a wait with the provider's own figure, and it says nothing about the
/// credential — an account marked invalid here would stop every other download under it.
#[tokio::test]
async fn a_rate_limit_waits_the_time_the_provider_stated() {
    let host = MockHost::new(
        answer(
            429,
            "/unrestrict/link",
            r#"{"error":"too many requests","error_code":34}"#,
            vec![ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }],
        ),
        true,
    );
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("a rate limit");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(90)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("realdebrid.rate_limited"));
}

/// The sign-in aged out. Reported as an invalid account so the interface offers a sign-in
/// rather than a retry, and with nothing the provider wrote in it.
#[tokio::test]
async fn an_expired_sign_in_invalidates_the_account_without_quoting_the_provider() {
    let host = MockHost::new(
        unrestrict(401, r#"{"error":"bad token AT-7f3c9","error_code":8}"#),
        true,
    );
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("a refusal");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("realdebrid.auth_invalid"));
    assert!(!failure.message.contains("AT-7f3c9"));
}

#[tokio::test]
async fn an_unavailable_file_is_reported_offline() {
    let host = MockHost::new(
        unrestrict(503, r#"{"error":"file unavailable","error_code":24}"#),
        true,
    );
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("a refusal");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("realdebrid.file_offline"));
}

/// An answer with no error and no address is not a success. Reporting one would queue a
/// download with nothing behind it.
#[tokio::test]
async fn an_answer_without_an_address_is_a_failure() {
    let host = MockHost::new(
        unrestrict(200, r#"{"id":"XYZ","filename":"release.rar"}"#),
        true,
    );
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("a refusal");
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("realdebrid.no_download_url"));
}

/// An account that was never signed in fails before a request is made, so no credential-less
/// call reaches the provider and nothing counts against the rate limit.
#[tokio::test]
async fn an_account_without_a_token_never_reaches_the_provider() {
    let host = MockHost::with_responses(Vec::new(), false);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("a refusal");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("realdebrid.token_missing"));
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn the_account_status_reads_the_plan_and_the_time_left_on_it() {
    let host = MockHost::new(
        answer(
            200,
            "/user",
            r#"{"id":1,"username":"alice","email":"a@test","points":300,"locale":"en",
                "type":"premium","premium":1209600,"expiration":"2026-10-01T00:00:00.000Z"}"#,
            Vec::new(),
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
        "plugin.account.user(user=alice)"
    );
    // No figure in bytes exists in the API, so none is invented from the loyalty points.
    assert_eq!(status.traffic_left, None);
}

#[tokio::test]
async fn an_expired_premium_plan_is_not_reported_as_premium() {
    let host = MockHost::new(
        answer(
            200,
            "/user",
            r#"{"id":1,"username":"alice","type":"premium","premium":0}"#,
            Vec::new(),
        ),
        true,
    );
    let status = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(!status.premium);
}

/// The catalogue is public, so the request that fetches it carries no credential.
#[tokio::test]
async fn the_hoster_catalogue_is_fetched_without_a_credential() {
    let host = MockHost::new(
        answer(
            200,
            "/hosts/domains",
            r#"["rapidgator.net","1FICHIER.COM","1fichier.com"]"#,
            Vec::new(),
        ),
        true,
    );
    let hosters = resolver(&host)
        .hosters(AccountId::new())
        .await
        .expect("hosters");
    assert_eq!(hosters, vec!["1fichier.com", "rapidgator.net"]);

    let requests = host.requests();
    let request = requests.first().expect("one request");
    assert!(
        request
            .headers
            .iter()
            .all(|header| header.name != "Authorization"),
        "a public catalogue must not be fetched with a credential"
    );
}

/// One refused link does not end the batch, and `supported` is not confused with `online`.
#[tokio::test]
async fn a_check_reports_each_link_and_carries_on_past_a_refusal() {
    let host = MockHost::with_responses(
        vec![
            answer(
                200,
                "/unrestrict/check",
                r#"{"host":"example.test","link":"https://example.test/f/one",
                    "filename":"one.rar","filesize":4096,"supported":1}"#,
                Vec::new(),
            ),
            answer(
                503,
                "/unrestrict/check",
                r#"{"error":"file unavailable","error_code":24}"#,
                Vec::new(),
            ),
            answer(
                200,
                "/unrestrict/check",
                r#"{"host":"other.test","link":"https://other.test/f/three",
                    "filename":"three.rar","filesize":8192,"supported":0}"#,
                Vec::new(),
            ),
        ],
        true,
    );
    let results = resolver(&host)
        .check(CheckRequest {
            urls: [
                "https://example.test/f/one",
                "https://example.test/f/two",
                "https://other.test/f/three",
            ]
            .iter()
            .map(|url| url.parse().expect("URL"))
            .collect(),
            client: client(),
        })
        .await
        .expect("checked");

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("one.rar"));
    assert_eq!(results[1].status, LinkStatus::Offline);
    // Present on a hoster the plan does not cover is still present.
    assert_eq!(results[2].status, LinkStatus::Online);
}

/// A rate limit in the middle of a batch stops it. Carrying on would spend refused requests
/// against the very cap that refused them.
#[tokio::test]
async fn a_rate_limit_stops_a_batch_rather_than_deepening_it() {
    let host = MockHost::with_responses(
        vec![answer(
            429,
            "/unrestrict/check",
            r#"{"error":"too many requests","error_code":34}"#,
            vec![ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "30".to_owned(),
            }],
        )],
        true,
    );
    let failure = resolver(&host)
        .check(CheckRequest {
            urls: ["https://example.test/f/one", "https://example.test/f/two"]
                .iter()
                .map(|url| url.parse().expect("URL"))
                .collect(),
            client: client(),
        })
        .await
        .expect_err("a rate limit");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(30)
        }
    );
    assert_eq!(host.requests().len(), 1);
}

/// The plugin claims what a multihoster claims, and not the magnet a torrent would need.
#[tokio::test]
async fn the_plugin_claims_http_addresses_and_not_magnets() {
    let host = MockHost::with_responses(Vec::new(), true);
    let resolver = resolver(&host);
    assert!(resolver.matches(&"https://example.test/f/abc".parse().expect("URL")));
    assert!(!resolver.matches(&"magnet:?xt=urn:btih:0123456789abcdef".parse().expect("URL")));
}

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
//! | Link check | the catalogue, `429`/34 included | `unknown` for a covered hoster, never a wait |
//!
//! A run against the real provider is deliberately **not** claimed here: it needs an account,
//! and the job file records which acceptance criteria that leaves unproven.

use std::sync::Arc;

use rd_core::{AccountId, FailureKind, LinkStatus};
use rd_plugin_api::test_support::ScriptedHost as MockHost;
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};

use super::RealDebridResolver;

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

fn check_request(urls: &[&str]) -> CheckRequest {
    CheckRequest {
        urls: urls.iter().map(|url| url.parse().expect("URL")).collect(),
        client: client(),
    }
}

fn catalogue(domains: &str) -> HostHttpResponse {
    answer(200, "/hosts/domains", domains, Vec::new())
}

/// A covered hoster is supported and unverified: `unknown`, from one catalogue request that
/// carries no credential, and never `unrestrict/check` (1.5.3).
#[tokio::test]
async fn a_check_reports_a_covered_hoster_as_unverified_from_the_catalogue() {
    let host = MockHost::new(catalogue(r#"["1fichier.com","rapidgator.net"]"#), true);
    let results = resolver(&host)
        .check(check_request(&[
            "https://1fichier.com/?abcdefghij0123456789",
            "https://rapidgator.net/file/abc",
        ]))
        .await
        .expect("checked");

    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|result| result.status == LinkStatus::Unknown)
    );
    let requests = host.requests();
    assert_eq!(requests.len(), 1, "one request for the whole batch");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://api.real-debrid.com/rest/1.0/hosts/domains"
    );
    assert!(
        requests[0]
            .headers
            .iter()
            .all(|header| header.name != "Authorization")
    );
}

/// The owner's 1.5.1 report: every check answered "rate limit reached, waiting". A `429` with
/// `error_code` 34 is no reason for a check to wait: the links come back `unknown`, and nothing
/// reaches the host that would hold the account or its downloads.
#[tokio::test]
async fn a_rate_limited_answer_leaves_the_links_unverified_rather_than_waiting() {
    let host = MockHost::new(
        answer(
            429,
            "/hosts/domains",
            r#"{"error":"too_many_requests","error_code":34}"#,
            vec![ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "30".to_owned(),
            }],
        ),
        true,
    );
    let results = resolver(&host)
        .check(check_request(&[
            "https://1fichier.com/?abcdefghij0123456789",
            "https://1fichier.com/?klmnopqrst9876543210",
        ]))
        .await
        .expect("a check never fails on a rate limit");

    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|result| result.status == LinkStatus::Unknown)
    );
    assert_eq!(host.requests().len(), 1, "no retry, no request per link");
}

/// A hoster the catalogue does not list is not Real-Debrid's to check.
#[tokio::test]
async fn a_check_of_an_uncovered_hoster_is_unsupported() {
    let host = MockHost::new(catalogue(r#"["1fichier.com"]"#), true);
    let failure = resolver(&host)
        .check(check_request(&["https://example.test/f/abc123"]))
        .await
        .expect_err("unsupported");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("realdebrid.host_unsupported"));
}

/// The plugin claims what a multihoster claims, and not the magnet a torrent would need.
#[tokio::test]
async fn the_plugin_claims_http_addresses_and_not_magnets() {
    let host = MockHost::with_responses(Vec::new(), true);
    let resolver = resolver(&host);
    assert!(resolver.matches(&"https://example.test/f/abc".parse().expect("URL")));
    assert!(!resolver.matches(&"magnet:?xt=urn:btih:0123456789abcdef".parse().expect("URL")));
}

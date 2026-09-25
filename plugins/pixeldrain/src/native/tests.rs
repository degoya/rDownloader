//! The resolver, driven against a mock of `pixeldrain.com/api`.
//!
//! No socket is opened, no real account is used and no request leaves the machine: the mock
//! answers at the host boundary, so a test also sees each request exactly as the plugin
//! described it. Every fixture body is sanitised -- invented identifiers, repeated-hex digests,
//! no address or account of a real person.
//!
//! The four cases the job asks for are `a_public_file_resolves_to_a_durable_address`,
//! `a_spent_allowance_is_a_wait_rather_than_a_download`, `an_authentication_refusal_is_reported`
//! and `a_missing_file_is_offline_rather_than_a_wait`.
//!
//! **A run against the live service is not claimed here.**
//! `docs/roadmap/jobs/120-07-pixeldrain.md` records that as open.

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

use super::PixeldrainResolver;

const FILE_URL: &str = "https://pixeldrain.com/u/Ab3xY9Zq";
const DIGEST: &str = "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12";

/// A quota answer with room in it: what the service says when nothing stands in the way.
const QUOTA_FREE: &str = r#"{"download_limit":300000000000,"download_limit_used":1048576,"transfer_limit":0,"transfer_limit_used":0,"speed_limit":0,"server_overload":false}"#;

struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    /// Whether an account's `pixeldrain_api_key` slot holds a value.
    has_key: bool,
}

impl MockHost {
    fn with_responses(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_key: false,
        })
    }

    /// The same mock for an account that holds an API key (RD-120-38).
    fn with_key(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_key: true,
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
        self.has_key && reference == "pixeldrain_api_key"
    }
}

fn answer(status: u16, path: &str, body: &str, retry_after: Option<&str>) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: format!("https://pixeldrain.com/api{path}")
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

fn file_info(status: u16, body: &str) -> HostHttpResponse {
    answer(status, "/file/Ab3xY9Zq/info", body, None)
}

fn quota(body: &str) -> HostHttpResponse {
    answer(200, "/misc/rate_limits", body, None)
}

fn resolver(host: &Arc<MockHost>) -> PixeldrainResolver {
    PixeldrainResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

fn client() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: FILE_URL.parse().expect("URL"),
        client: client(),
    }
}

#[tokio::test]
async fn a_public_file_resolves_to_a_durable_address() {
    let host = MockHost::with_responses(vec![
        file_info(
            200,
            &format!(
                r#"{{"id":"Ab3xY9Zq","name":"release.rar","size":4096,"mime_type":"application/x-rar","hash_sha256":"{DIGEST}","availability":""}}"#
            ),
        ),
        quota(QUOTA_FREE),
    ]);
    let resolved = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect("resolved");

    // The identifier is the whole address: no signature, no deadline, no session. A job that
    // waits an hour in the queue still has a working address when its turn comes, which is why
    // nothing here re-resolves per attempt.
    assert_eq!(
        resolved.url.as_str(),
        "https://pixeldrain.com/api/file/Ab3xY9Zq?download"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));

    let requests = host.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://pixeldrain.com/api/file/Ab3xY9Zq/info"
    );
    assert_eq!(
        requests[1].url.as_str(),
        "https://pixeldrain.com/api/misc/rate_limits"
    );
    // No account, no key, no cookie: a public file needs none and this plugin declares none.
    assert!(
        requests.iter().all(|request| request
            .headers
            .iter()
            .all(|header| header.name != "Authorization")),
        "nothing here carries a credential"
    );
}

#[tokio::test]
async fn a_missing_file_is_offline_rather_than_a_wait() {
    // The measured answer for an identifier the service never knew.
    let host = MockHost::with_responses(vec![file_info(
        404,
        r#"{"success":false,"value":"not_found","message":"The requested file does not exist, it may have been deleted."}"#,
    )]);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.file_not_found"));
    assert!(
        !failure.message.contains("may have been deleted"),
        "the provider's prose must not be forwarded"
    );
    // The quota endpoint is never reached: a file that is gone is the finer answer.
    assert_eq!(host.requests().len(), 1);
}

#[tokio::test]
async fn a_spent_allowance_is_a_wait_rather_than_a_download() {
    // The file is fine and this connection has spent its share. Reported, waited out, and not
    // worked around: no second address is asked for and nothing is retried in a loop.
    let host = MockHost::with_responses(vec![
        file_info(
            200,
            r#"{"id":"Ab3xY9Zq","name":"release.rar","size":4096,"availability":""}"#,
        ),
        quota(
            r#"{"download_limit":300000000000,"download_limit_used":300000000000,"server_overload":false}"#,
        ),
    ]);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.ip_rate_limited"));
}

#[tokio::test]
async fn a_429_on_the_metadata_carries_the_retry_after_the_service_asked_for() {
    let host = MockHost::with_responses(vec![answer(
        429,
        "/file/Ab3xY9Zq/info",
        r#"{"success":false,"value":"ip_rate_limit_reached","message":"x"}"#,
        Some("120"),
    )]);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.ip_rate_limited"));
    assert!(matches!(failure.category, FailureKind::IpBlocked { .. }));
}

#[tokio::test]
async fn an_authentication_refusal_is_reported() {
    // A file that is not public, asked for without an account: the only honest answer is to
    // name the obstacle.
    let host = MockHost::with_responses(vec![file_info(
        401,
        r#"{"success":false,"value":"authentication_required","message":"x"}"#,
    )]);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.account_required"));
}

#[tokio::test]
async fn a_blocked_file_is_permanent_and_never_reaches_the_quota_check() {
    let host = MockHost::with_responses(vec![file_info(
        200,
        r#"{"id":"Ab3xY9Zq","name":"release.rar","availability":"virus_detected_abuse"}"#,
    )]);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("refused");
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.file_blocked"));
    assert_eq!(host.requests().len(), 1);
}

#[tokio::test]
async fn an_unreadable_quota_answer_does_not_block_a_good_file() {
    // The quota call is a courtesy in front of a download that would otherwise fail later.
    // Letting an answer nobody can parse refuse a perfectly good file would be worse than the
    // 429 it guards against.
    let host = MockHost::with_responses(vec![
        file_info(200, r#"{"id":"Ab3xY9Zq","name":"a.bin","size":9}"#),
        quota("<html>maintenance</html>"),
    ]);
    let resolved = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect("resolved");
    assert_eq!(resolved.file_name.as_deref(), Some("a.bin"));
}

#[tokio::test]
async fn an_address_this_plugin_does_not_serve_is_walked_past() {
    // `unsupported` rather than a permanent failure, so the selection hands a list address to
    // `plugins/pixeldrain-crawler/` instead of ending the link here.
    let host = MockHost::with_responses(Vec::new());
    let failure = resolver(&host)
        .resolve(ResolveRequest {
            url: "https://pixeldrain.com/l/Ab3xY9Zq".parse().expect("URL"),
            client: client(),
        })
        .await
        .expect_err("refused");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.unsupported_link"));
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn a_link_check_reads_the_metadata_and_collapses_availability_deliberately() {
    // RD-120-36: `link-status` holds `online | offline | unknown`. A file behind a captcha
    // exists, so it stays online and the download attempt carries the obstacle; only a
    // moderation block is reported as gone.
    let host = MockHost::with_responses(vec![
        file_info(
            200,
            r#"{"id":"Ab3xY9Zq","name":"release.rar","size":4096,"availability":""}"#,
        ),
        file_info(
            200,
            r#"{"id":"Ab3xY9Zq","name":"b.bin","availability":"file_rate_limited_captcha_required"}"#,
        ),
        file_info(
            200,
            r#"{"id":"Ab3xY9Zq","name":"c.bin","availability":"virus_detected_abuse"}"#,
        ),
        file_info(
            404,
            r#"{"success":false,"value":"not_found","message":"x"}"#,
        ),
        answer(
            500,
            "/file/Ab3xY9Zq/info",
            r#"{"success":false,"value":"internal","message":"x"}"#,
            None,
        ),
    ]);
    let checks = resolver(&host)
        .check(CheckRequest {
            urls: vec![
                FILE_URL.parse().expect("URL"),
                FILE_URL.parse().expect("URL"),
                FILE_URL.parse().expect("URL"),
                FILE_URL.parse().expect("URL"),
                FILE_URL.parse().expect("URL"),
                // Not this plugin's address: unknown, and the batch carries on.
                "https://example.invalid/f/abc".parse().expect("URL"),
            ],
            client: client(),
        })
        .await
        .expect("checks");
    let statuses: Vec<LinkStatus> = checks.iter().map(|check| check.status).collect();
    assert_eq!(
        statuses,
        vec![
            LinkStatus::Online,
            LinkStatus::Online,
            LinkStatus::Offline,
            LinkStatus::Offline,
            // A server error says nothing about the file, so the row is not marked dead.
            LinkStatus::Unknown,
            LinkStatus::Unknown,
        ]
    );
    assert_eq!(checks[0].file_name.as_deref(), Some("release.rar"));
    assert_eq!(checks[0].size.map(|size| size.get()), Some(4096));
    assert_eq!(host.requests().len(), 5);
}

// -- an optional API key (RD-120-38) ------------------------------------------------------

const AUTHORIZATION_TEMPLATE: &str = "Basic {{basic:pixeldrain_api_key}}";

fn authorization_of(request: &HostHttpRequest) -> Option<&str> {
    request
        .headers
        .iter()
        .find(|header| header.name == "Authorization")
        .map(|header| header.value_template.as_str())
}

fn keyed_client() -> ClientIdentity {
    ClientIdentity {
        account_id: Some(AccountId::new()),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

/// With a key, every request of a resolve carries the template and never the key: the host
/// builds `base64(":key")`, and the quota asked about is the account's own.
#[tokio::test]
async fn a_resolve_with_a_key_sends_the_template_on_every_request() {
    let host = MockHost::with_key(vec![
        file_info(
            200,
            r#"{"id":"Ab3xY9Zq","name":"release.rar","size":4096,"availability":""}"#,
        ),
        quota(QUOTA_FREE),
    ]);
    let resolved = resolver(&host)
        .resolve(ResolveRequest {
            url: FILE_URL.parse().expect("URL"),
            client: keyed_client(),
        })
        .await
        .expect("resolved");
    // The transfer's credential is the engine's to attach (`transfer_auth = "basic"`); a
    // plugin that holds no key cannot state it as a download header.
    assert!(resolved.headers.is_empty());
    let requests = host.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(authorization_of(request), Some(AUTHORIZATION_TEMPLATE));
    }
}

/// An account with no key stored takes the free route rather than failing: sending the marker
/// would make the host refuse the request.
#[tokio::test]
async fn an_account_without_a_key_resolves_like_no_account() {
    let host = MockHost::with_responses(vec![
        file_info(200, r#"{"id":"Ab3xY9Zq","name":"release.rar","size":4096}"#),
        quota(QUOTA_FREE),
    ]);
    resolver(&host)
        .resolve(ResolveRequest {
            url: FILE_URL.parse().expect("URL"),
            client: keyed_client(),
        })
        .await
        .expect("resolved");
    assert!(
        host.requests()
            .iter()
            .all(|request| authorization_of(request).is_none())
    );
}

#[tokio::test]
async fn the_account_check_reads_the_subscription_behind_the_key() {
    for (body, premium) in [
        (
            r#"{"username":"someone","subscription":{"id":"prepaid","type":"prepaid"}}"#,
            true,
        ),
        (
            r#"{"username":"someone","subscription":{"id":"","type":""}}"#,
            false,
        ),
        (r#"{"username":"someone"}"#, false),
    ] {
        let host = MockHost::with_key(vec![answer(200, "/user", body, None)]);
        let account = resolver(&host)
            .check_account(AccountId::new())
            .await
            .expect("an account");
        assert!(account.valid);
        assert_eq!(account.premium, premium, "{body}");
        let request = &host.requests()[0];
        assert_eq!(request.url.as_str(), "https://pixeldrain.com/api/user");
        assert_eq!(authorization_of(request), Some(AUTHORIZATION_TEMPLATE));
    }
}

/// The measured answer for a key Pixeldrain does not accept (2026-09-23).
#[tokio::test]
async fn a_key_the_provider_refuses_is_the_accounts_fault() {
    let host = MockHost::with_key(vec![answer(
        401,
        "/user",
        r#"{"success":false,"value":"authentication_failed","message":"The provided API key is invalid, has been revoked or has expired"}"#,
        None,
    )]);
    let failure = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect_err("refused");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.api_key_invalid"));
    // The provider's sentence is not forwarded.
    assert!(!failure.message.contains("revoked or has expired"));
}

#[tokio::test]
async fn an_account_without_a_key_is_refused_before_a_request_goes_out() {
    let host = MockHost::with_responses(Vec::new());
    let failure = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect_err("no key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("pixeldrain.api_key_missing"));
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn the_manifest_is_the_one_authority_for_what_this_plugin_is() {
    let host = MockHost::with_responses(Vec::new());
    let resolver = resolver(&host);
    let metadata = resolver.metadata().clone();
    assert_eq!(metadata.provider_slug, "pixeldrain");
    assert!(
        !metadata.requires_account,
        "public files need no account at all"
    );
    assert_eq!(metadata.max_concurrent_downloads, 2);
    assert_eq!(
        resolver.hosters(AccountId::new()).await.expect("hosters"),
        vec!["pixeldrain.com".to_owned()]
    );
    assert!(resolver.matches(&FILE_URL.parse().expect("URL")));
    assert!(
        !resolver.matches(&"https://pixeldrain.com/l/Ab3xY9Zq".parse().expect("URL")),
        "a list address belongs to the crawler"
    );
}

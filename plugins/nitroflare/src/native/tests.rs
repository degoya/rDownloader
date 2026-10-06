use std::sync::Arc;

use rd_core::{AccountId, FailureKind, LinkStatus};
use rd_plugin_api::test_support::ScriptedHost as MockHost;
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};
use url::Url;

use super::NitroflareResolver;

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

fn file_info_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://nitroflare.com/api/v2/getFileInfo", body)
}

fn download_link_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://nitroflare.com/api/v2/getDownloadLink", body)
}

fn key_info_response(body: &str) -> HostHttpResponse {
    json_response(200, "https://nitroflare.com/api/v2/getKeyInfo", body)
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://nitroflare.com/view/ABCDEFGHIJ"
            .parse()
            .expect("URL"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
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
async fn happy_path_resolve_gets_file_info_then_download_link() {
    let host = MockHost::with_responses(
        vec![
            file_info_response(
                r#"{"result":{"files":{"ABCDEFGHIJ":{"status":"online","name":"release.rar","size":"4096"}}}}"#,
            ),
            download_link_response(
                r#"{"result":{"url":"https://cdn1.nitroflare.com/d/tok/release.rar"}}"#,
            ),
        ],
        true,
    );
    let resolver = NitroflareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://cdn1.nitroflare.com/d/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));
    assert_eq!(resolved.checksum, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2);

    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://nitroflare.com/api/v2/getFileInfo"
    );
    assert_eq!(query_value(&requests[0], "files"), Some("ABCDEFGHIJ"));
    // getFileInfo is unauthenticated: no user/premiumKey template.
    assert_eq!(query_value(&requests[0], "user"), None);
    assert_eq!(query_value(&requests[0], "premiumKey"), None);

    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].url.as_str(),
        "https://nitroflare.com/api/v2/getDownloadLink"
    );
    assert_eq!(query_value(&requests[1], "user"), Some("{{username}}"));
    assert_eq!(
        query_value(&requests[1], "premiumKey"),
        Some("{{secret:nitroflare_premium_key}}")
    );
    assert_eq!(query_value(&requests[1], "file"), Some("ABCDEFGHIJ"));
    // user, premiumKey, file — in that order.
    assert_eq!(
        requests[1]
            .query
            .iter()
            .map(|value| value.name.as_str())
            .collect::<Vec<_>>(),
        vec!["user", "premiumKey", "file"]
    );
}

#[tokio::test]
async fn resolve_without_premium_key_reports_premium_key_missing_and_sends_no_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = NitroflareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("nitroflare.premium_key_missing")
    );
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn resolve_with_bad_credentials_reports_account_invalid() {
    let host = MockHost::with_responses(
        vec![
            file_info_response(
                r#"{"result":{"files":{"ABCDEFGHIJ":{"status":"online","name":"release.rar","size":"4096"}}}}"#,
            ),
            download_link_response(r#"{"message":"Wrong login","code":8}"#),
        ],
        true,
    );
    let resolver = NitroflareResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("bad credentials");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("nitroflare.bad_credentials"));
}

#[tokio::test]
async fn resolve_offline_file_reports_offline() {
    let host = MockHost::new(file_info_response(r#"{"result":{"files":[]}}"#), true);
    let resolver = NitroflareResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("nitroflare.file_offline"));
}

#[tokio::test]
async fn resolve_traffic_exhausted_reports_rate_limited_with_retry_after() {
    let host = MockHost::with_responses(
        vec![
            file_info_response(
                r#"{"result":{"files":{"ABCDEFGHIJ":{"status":"online","name":"release.rar","size":"4096"}}}}"#,
            ),
            download_link_response(
                r#"{"message":"You have exceeded your daily traffic limit","code":99}"#,
            ),
        ],
        true,
    );
    let resolver = NitroflareResolver::new(host as Arc<dyn ResolverHost>);
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
        Some("nitroflare.traffic_exhausted")
    );
}

#[tokio::test]
async fn resolve_premium_only_file_reports_premium_required() {
    let host = MockHost::with_responses(
        vec![
            file_info_response(
                r#"{"result":{"files":{"ABCDEFGHIJ":{"status":"online","name":"release.rar","size":"4096"}}}}"#,
            ),
            download_link_response(r#"{"message":"Access denied","code":1}"#),
        ],
        true,
    );
    let resolver = NitroflareResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("premium required");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("nitroflare.premium_required"));
}

#[tokio::test]
async fn check_account_reports_premium_and_expiry_label() {
    let host = MockHost::new(
        key_info_response(
            r#"{"result":{"status":"active","trafficLeft":"2147483648","expiryDate":"2027-01-01 00:00:00"}}"#,
        ),
        true,
    );
    let resolver = NitroflareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.premium_until(until=2027-01-01 00:00:00)"
    );
    assert_eq!(
        status.traffic_left.map(|bytes| bytes.get()),
        Some(2_147_483_648)
    );

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[0].url.as_str(),
        "https://nitroflare.com/api/v2/getKeyInfo"
    );
    assert_eq!(query_value(&requests[0], "user"), Some("{{username}}"));
    assert_eq!(
        query_value(&requests[0], "premiumKey"),
        Some("{{secret:nitroflare_premium_key}}")
    );
}

#[tokio::test]
async fn check_account_expired_key_is_not_premium() {
    let host = MockHost::new(
        key_info_response(r#"{"result":{"status":"expired","expiryDate":"0"}}"#),
        true,
    );
    let resolver = NitroflareResolver::new(host as Arc<dyn ResolverHost>);
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
async fn check_account_without_premium_key_reports_premium_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = NitroflareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("nitroflare.premium_key_missing")
    );
    assert!(host.requests.lock().expect("mock lock").is_empty());
}

#[tokio::test]
async fn check_maps_online_offline_and_unparseable_batch() {
    let host = MockHost::new(
        file_info_response(
            r#"{"result":{"files":{"ABCDEFGHIJ":{"status":"online","name":"a.rar","size":"10"}}}}"#,
        ),
        true,
    );
    let resolver = NitroflareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                "https://nitroflare.com/view/ABCDEFGHIJ"
                    .parse::<Url>()
                    .expect("URL"),
                "https://nitroflare.com/view/KLMNOPQRST"
                    .parse::<Url>()
                    .expect("URL"),
                "https://nitroflare.com/member?s=api"
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
    // Its file id is absent from the getFileInfo response -> offline (JD treats a missing entry
    // as unavailable, same as an explicit non-"online" status).
    assert_eq!(results[1].status, LinkStatus::Offline);
    // Unparseable URL never reaches the batch call.
    assert_eq!(results[2].status, LinkStatus::Unknown);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        query_value(&requests[0], "files"),
        Some("ABCDEFGHIJ,KLMNOPQRST")
    );
}

#[tokio::test]
async fn check_degrades_batch_failure_to_unknown_instead_of_aborting() {
    let host = MockHost::new(
        file_info_response(r#"{"message":"Server error","code":500}"#),
        true,
    );
    let resolver = NitroflareResolver::new(host as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                "https://nitroflare.com/view/ABCDEFGHIJ"
                    .parse::<Url>()
                    .expect("URL"),
            ],
            client: client_identity(),
        })
        .await
        .expect("results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, LinkStatus::Unknown);
}

#[path = "free_tests.rs"]
mod free_tests;

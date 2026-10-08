use std::sync::Arc;

use rd_plugin_api::test_support::ScriptedHost as MockHost;
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver,
    ResolverHost,
};
use rd_plugin_types::{AccountId, FailureKind, LinkStatus};
use url::Url;

use super::OneFichierResolver;

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
    json_response(200, "https://api.1fichier.com/v1/file/info.cgi", body)
}

fn get_token_response(body: &str) -> HostHttpResponse {
    json_response(
        200,
        "https://api.1fichier.com/v1/download/get_token.cgi",
        body,
    )
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://1fichier.com/?abc12defg3".parse().expect("URL"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

#[tokio::test]
async fn happy_path_resolve_posts_file_info_then_get_token() {
    let host = MockHost::with_responses(
        vec![
            file_info_response(r#"{"filename":"release.rar","size":"4096"}"#),
            get_token_response(
                r#"{"status":"OK","url":"https://cdn123.1fichier.com/d/tok/release.rar"}"#,
            ),
        ],
        true,
    );
    let resolver = OneFichierResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://cdn123.1fichier.com/d/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));
    assert_eq!(resolved.checksum, None);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2);

    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].url.as_str(),
        "https://api.1fichier.com/v1/file/info.cgi"
    );
    assert_eq!(
        requests[0].body,
        br#"{"url":"https://1fichier.com/?abc12defg3"}"#
    );
    assert!(requests[0].headers.iter().any(|header| {
        header.name == "Authorization"
            && header.value_template == "Bearer {{secret:onefichier_api_key}}"
    }));
    assert!(requests[0].headers.iter().any(|header| {
        header.name == "Content-Type" && header.value_template == "application/json"
    }));

    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].url.as_str(),
        "https://api.1fichier.com/v1/download/get_token.cgi"
    );
    assert_eq!(
        requests[1].body,
        br#"{"url":"https://1fichier.com/?abc12defg3"}"#
    );
    assert!(requests[1].headers.iter().any(|header| {
        header.name == "Authorization"
            && header.value_template == "Bearer {{secret:onefichier_api_key}}"
    }));
}

#[tokio::test]
async fn resolve_without_api_key_reports_api_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("1fichier.api_key_missing"));
}

#[tokio::test]
async fn resolve_with_bad_api_key_reports_account_invalid() {
    let host = MockHost::new(
        file_info_response(r#"{"status":"KO","message":"Not authenticated #12"}"#),
        true,
    );
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("bad key");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("1fichier.bad_api_key"));
}

/// A 404 is the file deleted: final, not retried (owner, 2026-10-04), under the same code.
#[tokio::test]
async fn resolve_deleted_file_is_final_via_http_404() {
    let host = MockHost::new(
        json_response(
            404,
            "https://api.1fichier.com/v1/file/info.cgi",
            r#"{"status":"KO","message":"Resource not found #469"}"#,
        ),
        true,
    );
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("offline");
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("1fichier.file_offline"));
}

#[tokio::test]
async fn resolve_flood_detected_reports_rate_limited_with_retry_after() {
    let host = MockHost::new(
        file_info_response(r#"{"status":"KO","message":"Flood detected: IP Locked #38"}"#),
        true,
    );
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("flood");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("1fichier.flood"));
}

#[tokio::test]
async fn resolve_free_account_key_reports_premium_required() {
    let host = MockHost::with_responses(
        vec![
            file_info_response(r#"{"filename":"release.rar","size":"4096"}"#),
            get_token_response(
                r#"{"status":"KO","message":"Must be a customer (Premium, Access) #200"}"#,
            ),
        ],
        true,
    );
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("premium required");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("1fichier.premium_required"));
}

#[tokio::test]
async fn resolve_with_malformed_download_url_reports_invalid_url() {
    // Regression test: `download/get_token.cgi` answering with a syntactically invalid `url`
    // must fail resolve() identically on the native and guest adapters — both now delegate to
    // the shared `api::parse_download_url`, so this exercises that one tested path rather than
    // a native-only `Url::parse` call that the guest adapter used to skip entirely.
    let host = MockHost::with_responses(
        vec![
            file_info_response(r#"{"filename":"release.rar","size":"4096"}"#),
            get_token_response(r#"{"status":"OK","url":"not a url"}"#),
        ],
        true,
    );
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("malformed download URL");
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("1fichier.invalid_url"));
}

#[tokio::test]
async fn check_account_reports_premium_and_label() {
    let host = MockHost::new(
        json_response(
            200,
            "https://api.1fichier.com/v1/user/info.cgi",
            r#"{"email":"user@example.test","offer":1,"subscription_end":"2027-01-01","cdn":"2"}"#,
        ),
        true,
    );
    let resolver = OneFichierResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=user@example.test) plugin.account.premium_until(until=2027-01-01)"
    );
    assert_eq!(
        status.traffic_left.map(|bytes| bytes.get()),
        Some(2 * 1024 * 1024 * 1024)
    );

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[0].url.as_str(),
        "https://api.1fichier.com/v1/user/info.cgi"
    );
    assert_eq!(requests[0].body, b"{}");
}

#[tokio::test]
async fn check_account_free_offer_is_not_premium() {
    let host = MockHost::new(
        json_response(
            200,
            "https://api.1fichier.com/v1/user/info.cgi",
            r#"{"email":"user@example.test","offer":0}"#,
        ),
        true,
    );
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(!status.premium);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=user@example.test)"
    );
    assert_eq!(status.traffic_left, None);
}

#[tokio::test]
async fn check_account_without_api_key_reports_api_key_missing() {
    let host = MockHost::with_responses(Vec::new(), false);
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("1fichier.api_key_missing"));
}

#[tokio::test]
async fn check_maps_online_offline_and_unparseable_batch() {
    let host = MockHost::with_responses(
        vec![
            file_info_response(r#"{"filename":"a.rar","size":"10"}"#),
            json_response(
                404,
                "https://api.1fichier.com/v1/file/info.cgi",
                r#"{"status":"KO","message":"Resource not found #1"}"#,
            ),
        ],
        true,
    );
    let resolver = OneFichierResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                "https://1fichier.com/?abc12defg3"
                    .parse::<Url>()
                    .expect("URL"),
                "https://1fichier.com/?deadbeef99"
                    .parse::<Url>()
                    .expect("URL"),
                "https://1fichier.com/not-a-file-link"
                    .parse::<Url>()
                    .expect("URL"),
            ],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("results");

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("a.rar"));
    assert_eq!(results[0].size.map(|size| size.get()), Some(10));
    assert_eq!(results[1].status, LinkStatus::Offline);
    assert_eq!(results[2].status, LinkStatus::Unknown);

    // The unparseable third URL never triggers an API call.
    assert_eq!(host.requests.lock().expect("mock lock").len(), 2);
}

#[tokio::test]
async fn check_degrades_bad_key_to_unknown_instead_of_aborting_the_batch() {
    let host = MockHost::with_responses(
        vec![file_info_response(
            r#"{"status":"KO","message":"Not authenticated #12"}"#,
        )],
        true,
    );
    let resolver = OneFichierResolver::new(host as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                "https://1fichier.com/?abc12defg3"
                    .parse::<Url>()
                    .expect("URL"),
            ],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, LinkStatus::Unknown);
}

#[path = "free_tests.rs"]
mod free_tests;

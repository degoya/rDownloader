//! `check()` batch-mapping tests, split out of `tests.rs` to keep both files under the crate
//! layout's 500-line convention; same `MockHost` harness (re-used via `pub(crate)` items).

use std::sync::Arc;

use rd_core::{AccountId, LinkStatus};
use rd_plugin_api::{CheckRequest, ClientIdentity, Resolver, ResolverHost};

use super::super::KatfileResolver;
use super::{MockHost, json};

#[tokio::test]
async fn check_batches_file_info_and_maps_online_and_offline_status() {
    let response = json(
        "https://katfile.biz/api/file/info",
        br#"{"status":200,"msg":"OK","result":[{"status":200,"filecode":"abc123xyz","name":"release.rar","size":"2048"},{"status":404,"filecode":"deaddeadde"}]}"#,
    );
    let host = MockHost::new(response, true);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                "https://katfile.com/abc123xyz/release.rar"
                    .parse()
                    .expect("URL"),
                "https://katfile.com/deaddeadde".parse().expect("URL"),
            ],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("check succeeds");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("release.rar"));
    assert_eq!(results[1].status, LinkStatus::Offline);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .query
            .iter()
            .any(|q| q.name == "file_code" && q.value_template == "abc123xyz,deaddeadde")
    );
}

#[tokio::test]
async fn check_without_secret_reports_api_key_required_and_makes_no_requests() {
    let host = MockHost::bare(false, true);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check(CheckRequest {
            urls: vec!["https://katfile.com/abc123xyz".parse().expect("URL")],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect_err("missing API key must fail");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("katfile.api_key_required"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 0);
}

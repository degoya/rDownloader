//! "Re-resolve" at once: the files are resolved without a transfer and their size, name and
//! online status written back before the download starts (RD-1250-05).
//!
//! A tester's report: re-resolving only dropped the binding, so the size of a file was known once
//! its download began. Most cases go through a plain direct link on a local server, which the
//! LinkGrabber's probe answers, so no plugin component is needed.

use crate::common;

use axum::http::StatusCode;
use common::{parked_harness, post_json};
use serde_json::json;

/// A server with one file, `/file`, that names itself, and `/gone`, which answers 404.
async fn server() -> std::net::SocketAddr {
    let app = axum::Router::new()
        .route(
            "/file",
            axum::routing::get(|| async {
                (
                    [
                        ("content-type", "application/octet-stream"),
                        ("content-disposition", "attachment; filename=\"named.bin\""),
                    ],
                    vec![7_u8; 4_096],
                )
            }),
        )
        .route(
            "/gone",
            axum::routing::get(|| async { StatusCode::NOT_FOUND }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

/// One paused download of `url` in a package of its own, with no size known yet.
async fn paused_download(database: &rd_db::Database, url: String) -> rd_core::DownloadId {
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "Checked".to_owned(),
            destination: "downloads/Checked".to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: url.parse().expect("URL"),
            file_name: "download".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download")
        .id
}

#[tokio::test]
async fn re_resolving_now_writes_size_and_name_back_without_starting_the_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let address = server().await;
    let download = paused_download(&harness.database, format!("http://{address}/file")).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reresolve",
        json!({ "ids": [download], "check_now": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 1, "{body}");
    assert_eq!(body["resolved"], 1, "{body}");
    assert_eq!(body["failed"], 0, "{body}");
    assert_eq!(body["results"][0]["status"], "online", "{body}");
    assert_eq!(body["results"][0]["file_name"], "named.bin", "{body}");
    assert_eq!(body["results"][0]["total_bytes"], "4096", "{body}");

    let file = harness
        .database
        .get_download(download)
        .await
        .expect("read")
        .expect("download");
    assert_eq!(file.file_name, "named.bin");
    assert_eq!(file.total_bytes.map(rd_core::ByteCount::get), Some(4_096));
    assert_eq!(
        file.state,
        rd_core::DownloadState::Paused,
        "nothing was started"
    );
    assert_eq!(file.committed_bytes.get(), 0, "nothing was transferred");
}

#[tokio::test]
async fn a_file_the_server_no_longer_has_is_counted_as_failed_and_carries_the_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let address = server().await;
    let gone = paused_download(&harness.database, format!("http://{address}/gone")).await;
    let there = paused_download(&harness.database, format!("http://{address}/file")).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reresolve",
        json!({ "ids": [gone, there], "check_now": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["resolved"], 1, "{body}");
    assert_eq!(body["failed"], 1, "{body}");
    assert_eq!(
        body["results"][0]["download_id"],
        gone.to_string(),
        "{body}"
    );
    assert_eq!(body["results"][0]["status"], "offline", "{body}");
    assert_eq!(
        body["results"][0]["failure"]["code"], "download.resolve_offline",
        "{body}"
    );

    let file = harness
        .database
        .get_download(gone)
        .await
        .expect("read")
        .expect("download");
    assert_eq!(
        file.last_error.and_then(|failure| failure.code).as_deref(),
        Some("download.resolve_offline")
    );
    assert_eq!(
        file.state,
        rd_core::DownloadState::Paused,
        "the check decides no state"
    );
}

/// Without `check_now` the answer is the one it was: the binding is dropped, nothing resolved.
#[tokio::test]
async fn without_check_now_nothing_is_resolved_before_the_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let address = server().await;
    let download = paused_download(&harness.database, format!("http://{address}/file")).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reresolve",
        json!({ "ids": [download] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 1, "{body}");
    assert_eq!(body["results"], json!([]), "{body}");
    let file = harness
        .database
        .get_download(download)
        .await
        .expect("read")
        .expect("download");
    assert_eq!(file.file_name, "download");
    assert_eq!(file.total_bytes, None);
}

/// Owner, 2026-10-11: only links with an account are resolved ahead. A hoster link without one
/// would run the hoster's free flow (captcha, wait), so it is skipped, reported as such and left
/// untouched; nothing is asked of the hoster.
#[tokio::test]
async fn a_hoster_link_without_an_account_is_skipped_and_reported() {
    use rd_provider_registry::{
        CredentialKind, DynamicProvider, ProviderKind, ProviderSource, ProviderSpec, TransferAuth,
        try_register_dynamic,
    };
    const HOST: &str = "reresolve-skip.test";
    try_register_dynamic(DynamicProvider {
        plugin_id: "plugin-reresolveskip".to_owned(),
        spec: ProviderSpec {
            slug: "reresolveskip".to_owned(),
            display_name: HOST.to_owned(),
            kind: ProviderKind::Hoster,
            credentials: CredentialKind::ApiKey,
            username_required: false,
            transfer_auth: TransferAuth::None,
            secrets: Vec::new(),
            request_domains: vec![HOST.to_owned()],
            cookie_scope: None,
            match_hosts: vec![HOST.to_owned()],
            host_aliases: Vec::new(),
            source: ProviderSource::Plugin,
            plugin_id: Some("plugin-reresolveskip".to_owned()),
            plugin_version: Some("1.0.0".to_owned()),
        },
    })
    .expect("hoster registered");
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let download =
        paused_download(&harness.database, format!("https://{HOST}/abc123/file.rar")).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reresolve",
        json!({ "ids": [download], "check_now": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["affected"], 1,
        "the binding is dropped all the same: {body}"
    );
    assert_eq!(body["skipped"], 1, "{body}");
    assert_eq!(body["resolved"], 0, "{body}");
    assert_eq!(body["failed"], 0, "{body}");
    assert_eq!(body["results"][0]["skipped"], true, "{body}");
    assert_eq!(
        body["results"][0]["failure"]["code"], "download.resolve_skipped_free",
        "{body}"
    );
    let file = harness
        .database
        .get_download(download)
        .await
        .expect("read")
        .expect("download");
    assert_eq!(file.file_name, "download", "nothing written");
    assert!(file.last_error.is_none(), "nothing written");
}

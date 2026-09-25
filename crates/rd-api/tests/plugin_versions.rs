//! Removing the version an upgrade left behind, and refusing to remove one that is still in use.
//!
//! Installing a plugin never removes the older version: a job already under way keeps the
//! version that started it, which is the whole point of the resolver pin and of the transfer
//! checkpoint. Nothing ever cleared those leftovers, so the manager offers it now (RD-108-10) --
//! and has to answer for the case the leftover exists for.

mod common;

use axum::http::StatusCode;
use common::{delete_json, test_harness};

const PLUGIN: &str = "019d0000-0000-7000-8000-000000000108";

/// Two installed version directories, as an upgrade leaves them. The manifests are not read
/// here: removal works on the directory, and the listing skips what it cannot parse.
fn install_versions(directory: &std::path::Path, versions: &[&str]) {
    for version in versions {
        std::fs::create_dir_all(directory.join("plugins").join(PLUGIN).join(version))
            .expect("version directory");
    }
}

fn version_exists(directory: &std::path::Path, version: &str) -> bool {
    directory
        .join("plugins")
        .join(PLUGIN)
        .join(version)
        .is_dir()
}

/// Enqueues one download and pins it to `version`, the way resolving does.
async fn pinned_download(database: &rd_db::Database, version: &str) -> rd_core::DownloadId {
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "plugin versions".to_owned(),
            destination: "downloads".to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: "https://example.test/file.bin".parse().expect("URL"),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    database
        .claim_resolver_pin(
            download.id,
            rd_core::ResolverPin {
                plugin_id: PLUGIN.parse().expect("plugin id"),
                version: version.to_owned(),
            },
        )
        .await
        .expect("pin");
    download.id
}

#[tokio::test]
async fn a_version_a_job_is_pinned_to_is_not_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_versions(directory.path(), &["1.0.0", "2.0.0"]);
    let download = pinned_download(&harness.database, "1.0.0").await;

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.version_in_use");
    assert_eq!(
        body["params"]["count"], "1",
        "the refusal says how much is bound"
    );
    assert_eq!(
        body["params"]["names"], "file.bin",
        "and names the job in the way, so it can be found"
    );
    assert!(
        version_exists(directory.path(), "1.0.0"),
        "the package a queued job needs is still on disk"
    );

    // Cancelling is not the end of the job: `ProgressControl::cancel` keeps the partial data
    // and `resume` puts it back in the queue, so the version it would resume with stays.
    harness
        .database
        .transition_download(download, rd_core::DownloadState::Cancelled)
        .await
        .expect("cancel");

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.version_in_use");
    assert!(version_exists(directory.path(), "1.0.0"));

    // Deleting the job is what releases the claim, because deleting takes the pin with it.
    harness
        .database
        .delete_download(download)
        .await
        .expect("delete");

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !version_exists(directory.path(), "1.0.0"),
        "the leftover is gone"
    );
    assert!(
        version_exists(directory.path(), "2.0.0"),
        "the version that is loaded stays"
    );
}

#[tokio::test]
async fn the_version_that_is_loaded_is_guarded_the_same_way() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_versions(directory.path(), &["1.0.0", "2.0.0"]);
    pinned_download(&harness.database, "2.0.0").await;

    // Nothing is bound to the older one, so it goes.
    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The guard is about the version a job named, not about which one happens to be newest.
    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/2.0.0")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.version_in_use");
    assert!(version_exists(directory.path(), "2.0.0"));
}

#[tokio::test]
async fn removing_a_version_nothing_is_bound_to_still_answers_not_installed_twice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_versions(directory.path(), &["1.0.0"]);

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin.not_installed");
}

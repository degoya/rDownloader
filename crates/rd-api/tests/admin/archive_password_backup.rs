//! Archive passwords in the full backup (RD-190-04): sealed under the backup's own key in the
//! settings bundle, never in plain anywhere in the archive, and put back into the vault of the
//! machine a backup is restored on — another one included.

use axum::http::StatusCode;
use rd_backup::restore::cutover::{self, Cutover, Layout};
use serde_json::json;

use crate::common::{self, Harness};
use crate::full_restore::{PASSPHRASE, backed_up};

const PACKAGE_CANARY: &str = "rd190-archive-password-package-canary";
const GRABBER_CANARY: &str = "rd190-archive-password-grabber-canary";

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// A download package and a LinkGrabber package, each with its archive password.
async fn protected_packages(harness: &Harness, directory: &std::path::Path) -> rd_core::PackageId {
    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Protected".to_owned(),
            destination: directory.join("Protected").display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    // A package without a file is gone after the next start; this one has to outlive two.
    harness
        .database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: "https://example.com/Protected.part1.rar"
                .parse()
                .expect("url"),
            file_name: "Protected.part1.rar".to_owned(),
            total_bytes: rd_core::ByteCount::new(1024).ok(),
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
        .expect("a file for the package");
    harness
        .database
        .update_packages(
            vec![package.id],
            rd_db::PackageChange {
                password: Some(Some(PACKAGE_CANARY.to_owned())),
                ..rd_db::PackageChange::default()
            },
        )
        .await
        .expect("package password");
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/collector/batches",
        json!({
            "text": "https://ddownload.com/abc123/Release.part1.rar",
            "source": "api",
            "source_label": null,
            "package_name": "Release",
            "password": GRABBER_CANARY,
        }),
    )
    .await;
    assert!(status.is_success(), "{body}");
    package.id
}

/// Backed up on one machine, restored on another with a vault of its own: the archive holds
/// neither password in plain — not in its bytes, not in the database copy, not in the bundle —
/// and the restored installation reads both back from its own vault.
#[tokio::test]
async fn archive_passwords_travel_sealed_and_come_back_on_another_machine() {
    let origin = tempfile::tempdir().expect("tempdir");
    let source = common::test_harness(origin.path()).await;
    let package_id = protected_packages(&source, origin.path()).await;
    let (_, archive, _) = backed_up(&source, origin.path()).await;

    let raw = std::fs::read(&archive).expect("archive");
    assert!(!contains(&raw, PACKAGE_CANARY) && !contains(&raw, GRABBER_CANARY));
    let header = rd_backup::stream::read_header(&archive).expect("header");
    let key = rd_backup::BackupKey::derive(PASSPHRASE, header.salt)
        .await
        .expect("key");
    let opened = origin.path().join("opened");
    rd_backup::archive::extract_archive(&archive, &key, &opened).expect("extract");
    for part in [
        rd_backup::manifest::DATABASE_PART,
        rd_backup::manifest::SETTINGS_PART,
    ] {
        let bytes = std::fs::read(opened.join(part)).expect("part");
        assert!(
            !contains(&bytes, PACKAGE_CANARY) && !contains(&bytes, GRABBER_CANARY),
            "a password is in plain in {part}"
        );
    }
    let bundle: serde_json::Value = serde_json::from_slice(
        &std::fs::read(opened.join(rd_backup::manifest::SETTINGS_PART)).expect("bundle"),
    )
    .expect("json");
    let tables: Vec<&str> = bundle["archive_passwords"]
        .as_array()
        .expect("the bundle carries the archive passwords")
        .iter()
        .filter_map(|entry| entry["table"].as_str())
        .collect();
    assert!(
        tables.contains(&"packages") && tables.contains(&"collector_packages"),
        "{tables:?}"
    );
    assert!(
        bundle["secrets"].is_object(),
        "sealed under the backup's key"
    );

    // A settings export is about settings: it never carries an archive password.
    let (status, exported) = common::post_json(
        &source.router,
        "/api/v1/settings/export",
        json!({ "include_secrets": true, "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{exported}");
    assert!(exported.get("archive_passwords").is_none(), "{exported}");

    // Another machine: its own data directory and its own vault.
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let target = common::test_harness(elsewhere.path()).await;
    let request = json!({
        "source": { "path": archive.display().to_string() },
        "passphrase": PASSPHRASE,
    });
    let (status, report) = common::post_json(
        &target.router,
        "/api/v1/backups/restore/test",
        request.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["ok"], true, "{report}");
    let problems = report["problems"].to_string();
    assert!(!problems.contains("password_ref"), "{problems}");

    let (status, body) =
        common::post_json(&target.router, "/api/v1/backups/restore", request).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let layout = Layout::new(target.database.path());
    target.database.close().await.expect("close the database");
    drop(target);
    let outcome = cutover::apply_pending(&layout).expect("start");
    assert!(matches!(outcome, Cutover::Switched(_)), "{outcome:?}");
    let target = common::test_harness(elsewhere.path()).await;
    assert_eq!(
        target
            .database
            .package_password(package_id)
            .await
            .expect("password")
            .as_deref(),
        Some(PACKAGE_CANARY)
    );
    let grabbed = target
        .database
        .list_collector_packages()
        .await
        .expect("LinkGrabber packages");
    assert!(
        grabbed
            .iter()
            .any(|package| package.password.as_deref() == Some(GRABBER_CANARY)),
        "the LinkGrabber package lost its password"
    );
    cutover::finish(&layout).expect("finish");
}

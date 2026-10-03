//! The full backup's destinations through the REST surface (RD-160-02): every destination gets
//! its own copy, one that fails costs only its own row, retention removes only this
//! installation's recorded archives and can be previewed first, and a verification finds a
//! changed archive; a scheduled one that does is announced (RD-190-19).

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use crate::common::{self, Harness};
use crate::full_backup::add_folder;

const PASSPHRASE: &str = "correct horse battery staple";

pub(crate) async fn ready(harness: &Harness) {
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

pub(crate) async fn run(harness: &Harness) -> rd_db::BackupRun {
    let (status, run) = common::post_json(&harness.router, "/api/v1/backups/runs", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    let id = run["id"].as_str().expect("run id").to_owned();
    common::eventually(
        Duration::from_secs(60),
        "the backup run to finish",
        || async {
            harness
                .database
                .backup_run(&id)
                .await
                .expect("read run")
                .filter(|run| run.state != rd_core::BackupRunState::Running)
        },
    )
    .await
}

fn archives_in(folder: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .expect("folder")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".rdbackup"))
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_destination_is_checked_before_it_is_saved() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    for (request, code) in [
        (
            json!({ "kind": "webdav", "remote": "x" }),
            "backup.destination_kind_unknown",
        ),
        (
            json!({ "kind": "local", "path": "relative/folder" }),
            "storage_root.path_not_absolute",
        ),
        (
            json!({ "kind": "local" }),
            "backup.destination_path_missing",
        ),
        (
            json!({ "kind": "rclone", "remote": "no-colon" }),
            "backup.rclone_remote_invalid",
        ),
        (
            json!({ "kind": "object_storage", "prefix": "b" }),
            "backup.destination_profile_missing",
        ),
        (
            json!({ "kind": "object_storage", "profile_id": "404", "prefix": "b" }),
            "object_storage.profile_missing",
        ),
        (
            json!({ "kind": "local", "path": directory.path().join("x").display().to_string(), "keep_last": 0 }),
            "backup.retention_invalid",
        ),
    ] {
        let (status, body) =
            common::post_json(&harness.router, "/api/v1/backups/destinations", request).await;
        assert!(status.is_client_error(), "{status} {body}");
        assert_eq!(body["code"], code, "{body}");
    }

    // An rclone remote needs no rclone to be saved; a run names it missing.
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/destinations",
        json!({ "kind": "rclone", "remote": "webdav:backups", "keep_days": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["remote"], "webdav:backups");
    assert_eq!(body["keep_days"], 30);
    let id = body["id"].as_str().expect("id").to_owned();
    let (status, _) = common::delete_json(
        &harness.router,
        &format!("/api/v1/backups/destinations/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn every_destination_gets_a_copy_and_one_outage_costs_only_its_row() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    ready(&harness).await;
    let nas = directory.path().join("nas");
    let usb = directory.path().join("usb");
    add_folder(&harness, &nas).await;
    add_folder(&harness, &usb).await;

    let both = run(&harness).await;
    assert_eq!(
        both.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        both.error_detail
    );
    assert_eq!(both.destinations.len(), 2);
    assert_eq!(archives_in(&nas), archives_in(&usb));
    assert_eq!(archives_in(&nas).len(), 1);

    // The USB disk is gone and a file sits where it was mounted.
    std::fs::remove_dir_all(&usb).expect("unplug");
    std::fs::write(&usb, b"not a folder").expect("occupy");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let partial = run(&harness).await;
    assert_eq!(partial.state, rd_core::BackupRunState::Succeeded);
    assert_eq!(
        partial.error_code.as_deref(),
        Some("backup.destinations_partial")
    );
    let failed: Vec<_> = partial
        .destinations
        .iter()
        .filter(|row| row.state == rd_core::BackupRunState::Failed)
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(
        failed[0].error_code.as_deref(),
        Some("storage_root.path_is_file")
    );
    assert_eq!(archives_in(&nas).len(), 2);

    let (_, runs) = common::get_json(&harness.router, "/api/v1/backups/runs").await;
    assert_eq!(runs[0]["destinations"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn retention_removes_only_this_installation_s_recorded_archives() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    ready(&harness).await;
    let nas = directory.path().join("nas");
    let id = add_folder(&harness, &nas).await;
    // What else lies in the folder: somebody's file, another installation's archive, and one
    // named like this installation's that it never wrote.
    let instance = harness
        .database
        .backup_config()
        .await
        .expect("config")
        .instance_id;
    let strangers = [
        "holiday.rdbackup".to_owned(),
        "rdownloader-backup-ffffffff-20200101T000000Z.rdbackup".to_owned(),
        format!("rdownloader-backup-{instance}-20200101T000000Z.rdbackup"),
    ];
    for name in &strangers {
        std::fs::write(nas.join(name), b"not ours").expect("stranger");
    }

    let first = run(&harness).await;
    assert_eq!(
        first.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        first.error_detail
    );
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let second = run(&harness).await;
    assert_eq!(second.state, rd_core::BackupRunState::Succeeded);

    // The preview with one archive kept removes the older one, and nothing is deleted by it.
    let (status, preview) = common::get_json(
        &harness.router,
        &format!("/api/v1/backups/destinations/{id}/retention?keep_last=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["keep"].as_array().map(Vec::len), Some(1));
    assert_eq!(preview["remove"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        preview["remove"][0]["archive_name"],
        first.archive_name.clone().expect("name")
    );
    assert!(
        nas.join(first.archive_name.as_deref().expect("name"))
            .exists()
    );

    // Saved, the next run applies it.
    let (status, body) = common::put_json(
        &harness.router,
        &format!("/api/v1/backups/destinations/{id}"),
        json!({ "kind": "local", "path": nas.display().to_string(), "keep_last": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let third = run(&harness).await;
    assert_eq!(third.destinations[0].pruned, 2);
    let left = archives_in(&nas);
    assert!(left.contains(&third.archive_name.clone().expect("name")));
    assert!(!left.contains(&first.archive_name.clone().expect("name")));
    assert!(!left.contains(&second.archive_name.clone().expect("name")));
    for name in &strangers {
        assert!(nas.join(name).exists(), "{name} was deleted");
    }
    let (_, archives) = common::get_json(
        &harness.router,
        &format!("/api/v1/backups/archives?destination_id={id}"),
    )
    .await;
    assert_eq!(archives.as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn a_verification_passes_an_intact_archive_and_fails_a_changed_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    ready(&harness).await;
    let nas = directory.path().join("nas");
    add_folder(&harness, &nas).await;
    let written = run(&harness).await;
    assert_eq!(
        written.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        written.error_detail
    );
    let (_, archives) = common::get_json(&harness.router, "/api/v1/backups/archives").await;
    let archive_id = archives[0]["id"].as_str().expect("archive id").to_owned();

    let verify = || async {
        let (status, started) = common::post_json(
            &harness.router,
            &format!("/api/v1/backups/archives/{archive_id}/verify"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let id = started["id"].as_str().expect("verification id").to_owned();
        common::eventually(
            Duration::from_secs(60),
            "the verification to finish",
            || async {
                harness
                    .database
                    .backup_verifications(10)
                    .await
                    .expect("history")
                    .into_iter()
                    .find(|row| row.id == id && row.state != rd_core::BackupVerifyState::Running)
            },
        )
        .await
    };

    let passed = verify().await;
    assert_eq!(
        passed.state,
        rd_core::BackupVerifyState::Passed,
        "{:?}",
        passed.error_detail
    );
    assert_eq!(passed.content_checked, Some(true));

    let stored = nas.join(written.archive_name.as_deref().expect("name"));
    let mut bytes = std::fs::read(&stored).expect("archive");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x20;
    std::fs::write(&stored, &bytes).expect("tamper");
    let failed = verify().await;
    assert_eq!(failed.state, rd_core::BackupVerifyState::Failed);
    assert_eq!(
        failed.error_code.as_deref(),
        Some("backup.verify_digest_mismatch")
    );

    let (_, history) = common::get_json(&harness.router, "/api/v1/backups/verifications").await;
    assert_eq!(history.as_array().map(Vec::len), Some(2));
    let (_, archives) = common::get_json(&harness.router, "/api/v1/backups/archives").await;
    assert_eq!(archives[0]["verify_state"], "failed");
}

/// A scheduled verification that finds a changed archive reaches a rule asking for
/// `backup_verify_failed` (RD-190-19).
#[tokio::test]
async fn a_failed_scheduled_verification_is_announced() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (status, target) = common::post_json(
        &harness.router,
        "/api/v1/notifications/targets",
        json!({ "name": "hook", "kind": "webhook", "endpoint": "http://127.0.0.1:9/hook" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{target}");
    let (status, rule) = common::post_json(
        &harness.router,
        "/api/v1/notifications/rules",
        json!({ "name": "checks", "target_id": target["id"], "events": ["backup_verify_failed"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    ready(&harness).await;
    let nas = directory.path().join("nas");
    add_folder(&harness, &nas).await;
    let written = run(&harness).await;
    assert_eq!(
        written.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        written.error_detail
    );
    let stored = nas.join(written.archive_name.as_deref().expect("name"));
    let mut bytes = std::fs::read(&stored).expect("archive");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x20;
    std::fs::write(&stored, &bytes).expect("tamper");

    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups",
        json!({
            "enabled": false,
            "schedule": "0 3 * * *",
            "timezone": "Europe/Berlin",
            "verify_schedule": "0 4 * * *",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let due = harness
        .database
        .backup_config()
        .await
        .expect("config")
        .verify_next_run_at
        .expect("armed");
    assert_eq!(
        rd_api_admin::backup_verify_service::tick(
            &harness.state,
            due + chrono::Duration::minutes(1)
        )
        .await,
        1
    );

    let database = &harness.database;
    let deliveries = common::eventually(
        Duration::from_secs(60),
        "no backup_verify_failed delivery was queued",
        || async move {
            let deliveries = database
                .list_notification_deliveries(100)
                .await
                .expect("deliveries");
            (!deliveries.is_empty()).then_some(deliveries)
        },
    )
    .await;
    assert_eq!(deliveries.len(), 1, "{deliveries:?}");
    assert_eq!(
        deliveries[0].event,
        rd_notify::NotificationEvent::BackupVerifyFailed
    );
    assert!(
        deliveries[0].body.contains("backup.verify_digest_mismatch"),
        "{deliveries:?}"
    );
}

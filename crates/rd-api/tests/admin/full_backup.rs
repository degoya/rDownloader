//! Scheduled encrypted full backups through the REST surface (RD-160-01): the passphrase goes
//! in once and never comes out, replacing it asks for the current one, no backup is written without it, the schedule survives a
//! restart and runs once when due, a failure lands in the history and leaves the queue alone,
//! and the settings bundle inside an archive restores with the same passphrase.

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use crate::common::{self, Harness};

const PASSPHRASE: &str = "correct horse battery staple";

async fn set_passphrase(harness: &Harness) -> serde_json::Value {
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Adds a local folder as a destination (RD-160-02) and returns its id.
pub(crate) async fn add_folder(harness: &Harness, folder: &std::path::Path) -> String {
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/destinations",
        json!({ "kind": "local", "path": folder.display().to_string() }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().expect("destination id").to_owned()
}

async fn configure(harness: &Harness, enabled: bool) -> (StatusCode, serde_json::Value) {
    common::put_json(
        &harness.router,
        "/api/v1/backups",
        json!({
            "enabled": enabled,
            "schedule": "0 3 * * *",
            "timezone": "Europe/Berlin",
        }),
    )
    .await
}

/// Waits until the run is no longer `running` and returns its row.
async fn finished(harness: &Harness, id: &str) -> rd_db::BackupRun {
    common::eventually(
        Duration::from_secs(60),
        "the backup run to finish",
        || async {
            harness
                .database
                .backup_run(id)
                .await
                .expect("read run")
                .filter(|run| run.state != rd_core::BackupRunState::Running)
        },
    )
    .await
}

async fn start_by_hand(harness: &Harness) -> String {
    let (status, run) = common::post_json(&harness.router, "/api/v1/backups/runs", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    assert_eq!(run["origin"], "manual");
    run["id"].as_str().expect("run id").to_owned()
}

#[tokio::test]
async fn the_passphrase_goes_in_once_and_only_its_fingerprint_comes_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": "short" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "backup.passphrase_too_short");

    let body = set_passphrase(&harness).await;
    assert_eq!(body["key_configured"], true);
    let fingerprint = body["key_fingerprint"].as_str().expect("fingerprint");
    assert_eq!(fingerprint.len(), 16);
    let (_, config) = common::get_json(&harness.router, "/api/v1/backups").await;
    let text = config.to_string();
    assert!(!text.contains(PASSPHRASE));
    assert!(
        !text.contains("vault://"),
        "the key's reference leaked: {text}"
    );

    // The key is in the secret store and nowhere else: not in the settings document.
    let stored = harness.database.backup_config().await.expect("config");
    let reference = stored.key.expect("key").reference;
    assert!(harness.secrets.get_bytes(&reference).await.is_ok());
    let (_, settings) = common::get_json(&harness.router, "/api/v1/settings").await;
    assert!(!settings.to_string().contains(&reference));

    // Replacing it asks for the current one (owner's decision, 2026-09-28): without it, and
    // with a wrong one, nothing changes.
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": "another long passphrase" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "backup.passphrase_current_required");
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({
            "passphrase": "another long passphrase",
            "current_passphrase": "not the passphrase at all",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "backup.passphrase_wrong");
    let unchanged = harness.database.backup_config().await.expect("config");
    assert_eq!(unchanged.key.expect("key").reference, reference);

    // With the right one the key is replaced and the old one leaves the store.
    let (status, changed) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": "another long passphrase", "current_passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_ne!(changed["key_fingerprint"], fingerprint);
    assert!(harness.secrets.get_bytes(&reference).await.is_err());

    let audit = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    let changes: Vec<_> = audit
        .iter()
        .filter(|record| record.action == rd_core::AuditAction::BackupKeyChanged)
        .collect();
    // The first setup, the refused wrong passphrase and the change; the refusal as a failure.
    assert_eq!(changes.len(), 3);
    assert_eq!(
        changes
            .iter()
            .filter(|record| record.outcome == rd_core::AuditOutcome::Failure)
            .count(),
        1
    );
    let recorded = serde_json::to_string(&audit).expect("json");
    assert!(!recorded.contains(PASSPHRASE));
    assert!(!recorded.contains("not the passphrase at all"));
    assert!(!recorded.contains("another long passphrase"));
}

#[tokio::test]
async fn no_backup_is_written_without_a_passphrase() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let folder = directory.path().join("nas");
    add_folder(&harness, &folder).await;

    // The schedule cannot be switched on without a key.
    let (status, body) = configure(&harness, true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "backup.key_missing");

    // A run by hand is recorded as failed, and nothing reaches the folder.
    let (status, _) = configure(&harness, false).await;
    assert_eq!(status, StatusCode::OK);
    let id = start_by_hand(&harness).await;
    let run = finished(&harness, &id).await;
    assert_eq!(run.state, rd_core::BackupRunState::Failed);
    assert_eq!(run.error_code.as_deref(), Some("backup.key_missing"));
    assert_eq!(std::fs::read_dir(&folder).expect("folder").count(), 0);
}

#[tokio::test]
async fn a_backup_restores_its_settings_with_the_same_passphrase() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let secret = harness
        .secrets
        .put_string("proxy-password-canary".to_owned())
        .await
        .expect("secret");
    harness
        .database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "SOCKS".to_owned(),
            kind: rd_core::ProxyKind::Socks5,
            endpoint: "socks5h://127.0.0.1:1080".parse().expect("proxy URL"),
            username: Some("proxy-user".to_owned()),
            secret_ref: Some(secret),
        })
        .await
        .expect("proxy");
    set_passphrase(&harness).await;
    let folder = directory.path().join("nas");
    add_folder(&harness, &folder).await;
    let (status, _) = configure(&harness, true).await;
    assert_eq!(status, StatusCode::OK);

    let id = start_by_hand(&harness).await;
    let run = finished(&harness, &id).await;
    assert_eq!(
        run.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        run.error_detail
    );
    let archive = folder.join(run.archive_name.as_deref().expect("archive name"));
    let raw = std::fs::read(&archive).expect("archive");
    assert!(
        !raw.windows(b"proxy-password-canary".len())
            .any(|window| window == b"proxy-password-canary")
    );
    assert!(!directory.path().join("backup-staging").exists());

    // The restore asks for the passphrase and derives the key from the archive's header.
    let header = rd_backup::stream::read_header(&archive).expect("header");
    let key = rd_backup::BackupKey::derive(PASSPHRASE, header.salt)
        .await
        .expect("key");
    let opened = directory.path().join("opened");
    let manifest = rd_backup::archive::extract_archive(&archive, &key, &opened).expect("extract");
    assert_eq!(
        manifest.parts.len(),
        run.parts.expect("parts").as_array().expect("list").len()
    );
    let bundle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(opened.join("settings.json")).expect("bundle"))
            .expect("json");
    assert!(
        bundle["secrets"].is_object(),
        "the credentials are sealed in the bundle"
    );
    assert!(!bundle.to_string().contains("proxy-password-canary"));

    // The bundle goes back through the ordinary import with the same passphrase.
    let (status, summary) = common::post_json(
        &harness.router,
        "/api/v1/settings/import",
        json!({ "bundle": bundle, "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let proxies = harness
        .database
        .list_proxy_profiles()
        .await
        .expect("proxies");
    let restored = proxies[0].secret_ref.clone().expect("secret reference");
    let value = harness.secrets.get(&restored).await.expect("secret");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&value),
        "proxy-password-canary"
    );
}

#[tokio::test]
async fn a_scheduled_backup_runs_once_when_due_and_its_time_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    set_passphrase(&harness).await;
    let folder = directory.path().join("nas");
    add_folder(&harness, &folder).await;
    let (status, body) = configure(&harness, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let due = harness
        .database
        .backup_config()
        .await
        .expect("config")
        .next_run_at
        .expect("armed");
    assert!(due > chrono::Utc::now());

    // Not yet due: nothing.
    assert_eq!(
        rd_api::backup_service::tick(&harness.state, due - chrono::Duration::minutes(1)).await,
        None
    );
    // Due: one run, and the due time moves on before it starts.
    let now = due + chrono::Duration::minutes(1);
    let id = rd_api::backup_service::tick(&harness.state, now)
        .await
        .expect("a run");
    let advanced = harness
        .database
        .backup_config()
        .await
        .expect("config")
        .next_run_at
        .expect("armed");
    assert_eq!(
        advanced,
        rd_backup::schedule::next_run("0 3 * * *", "Europe/Berlin", now).expect("next")
    );
    assert!(advanced > now);
    assert_eq!(
        rd_api::backup_service::tick(&harness.state, now).await,
        None
    );
    let run = finished(&harness, &id).await;
    assert_eq!(
        run.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        run.error_detail
    );
    assert_eq!(run.origin, rd_core::BackupOrigin::Scheduled);

    // A restart reads the stored time back rather than timing the schedule anew.
    drop(harness);
    let harness = common::test_harness(directory.path()).await;
    let config = harness.database.backup_config().await.expect("config");
    assert_eq!(config.next_run_at, Some(advanced));
    assert_eq!(
        rd_api::backup_service::tick(&harness.state, now).await,
        None
    );
}

#[tokio::test]
async fn a_failed_backup_is_recorded_and_the_queue_goes_on() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": "https://example.invalid/before.mkv", "package_name": "Before" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    set_passphrase(&harness).await;
    let folder = directory.path().join("nas");
    add_folder(&harness, &folder).await;
    let (status, _) = configure(&harness, true).await;
    assert_eq!(status, StatusCode::OK);
    // The NAS goes away and something else takes its place.
    std::fs::remove_dir_all(&folder).expect("remove folder");
    std::fs::write(&folder, b"not a folder").expect("occupy");

    let id = start_by_hand(&harness).await;
    let run = finished(&harness, &id).await;
    assert_eq!(run.state, rd_core::BackupRunState::Failed);
    assert_eq!(run.error_code.as_deref(), Some("storage_root.path_is_file"));
    let (_, runs) = common::get_json(&harness.router, "/api/v1/backups/runs").await;
    assert_eq!(runs[0]["state"], "failed");
    assert_eq!(runs[0]["error_code"], "storage_root.path_is_file");

    // The queue did not notice: the download is where it was, and new work is taken.
    let (_, downloads) = common::get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(downloads.as_array().map(Vec::len), Some(1), "{downloads}");
    assert_eq!(downloads[0]["state"], "queued");
    let (status, _) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": "https://example.invalid/after.mkv", "package_name": "After" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // And a second run while none is going is accepted again.
    let (status, again) =
        common::post_json(&harness.router, "/api/v1/backups/runs", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{again}");
}

//! Restoring a full backup through the REST surface (RD-160-03): every step asks for the
//! passphrase the archive was made with and never uses the stored key, a preview and a test
//! restore leave the service as it was, a restore is staged with its credentials in the secret
//! store and becomes the installation at the next start, a staged restore can be discarded,
//! and an archive arrives in chunks at their offsets.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{StatusCode, header};
use rd_backup::restore::cutover::{self, Cutover, Layout};
use serde_json::json;

use crate::common::{self, Harness};

pub(crate) const PASSPHRASE: &str = "correct horse battery staple";
const CANARY: &str = "proxy-password-canary";

/// A proxy with a credential, a storage root, a passphrase and one finished backup of it all.
pub(crate) async fn backed_up(harness: &Harness, directory: &Path) -> (String, PathBuf, String) {
    let secret = harness
        .secrets
        .put_string(CANARY.to_owned())
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
    let root_id = rd_core::StorageRootId::new();
    harness
        .database
        .create_storage_root(
            root_id,
            rd_db::NewStorageRoot {
                name: "Restore root".to_owned(),
                path: directory.join("root").display().to_string(),
                is_default: false,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let folder = directory.join("nas");
    std::fs::create_dir_all(&folder).expect("destination folder");
    crate::full_backup::add_folder(harness, &folder).await;
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups",
        json!({
            "enabled": false,
            "schedule": "0 3 * * *",
            "timezone": "UTC",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, run) = common::post_json(&harness.router, "/api/v1/backups/runs", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    let id = run["id"].as_str().expect("run id").to_owned();
    let run = common::eventually(
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
    .await;
    assert_eq!(
        run.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        run.error_detail
    );
    let archive = folder.join(run.archive_name.expect("archive"));
    (id, archive, root_id.to_string())
}

fn files_below(folder: &Path) -> usize {
    std::fs::read_dir(folder).map_or(0, |entries| entries.count())
}

#[tokio::test]
async fn a_preview_opens_with_the_archives_passphrase_only_and_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (run_id, _, root_id) = backed_up(&harness, directory.path()).await;
    // The stored key moves on to another passphrase; the archive keeps the one it was made with.
    let (status, body) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": "another long passphrase", "current_passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let layout = Layout::new(harness.database.path());
    let secrets_before = files_below(&directory.path().join("secrets"));

    let preview =
        |passphrase: &str| json!({ "source": { "run_id": run_id }, "passphrase": passphrase });
    // The stored key's passphrase does not open an archive made before it.
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        preview("another long passphrase"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "backup.restore_passphrase_wrong");
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        json!({ "source": { "run_id": run_id }, "passphrase": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "backup.restore_passphrase_required");

    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        preview(PASSPHRASE),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["app_version"], body["current_version"]);
    assert_eq!(body["from_newer_version"], false);
    assert_eq!(body["proxy_profiles"], 1);
    assert_eq!(body["credentials_included"], true);
    let kinds: Vec<&str> = body["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .filter_map(|part| part["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"database") && kinds.contains(&"settings"),
        "{kinds:?}"
    );
    assert!(
        body["storage_roots"]
            .as_array()
            .expect("roots")
            .iter()
            .any(|root| root["id"] == root_id.as_str() && root["native"] == true)
    );
    let text = body.to_string();
    assert!(!text.contains(PASSPHRASE) && !text.contains(CANARY));

    // Nothing changed: no work folder, no restore waiting, nothing new in the secret store.
    assert!(files_below(&layout.work()) == 0);
    assert!(cutover::read_marker(&layout).expect("marker").is_none());
    assert_eq!(
        files_below(&directory.path().join("secrets")),
        secrets_before
    );

    // The wrong passphrase was audited as a failure, without either passphrase.
    let audit = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    assert!(audit.iter().any(|record| {
        record.action == rd_core::AuditAction::BackupRestored
            && record.outcome == rd_core::AuditOutcome::Failure
    }));
    let recorded = serde_json::to_string(&audit).expect("json");
    assert!(!recorded.contains(PASSPHRASE) && !recorded.contains("another long passphrase"));
}

#[tokio::test]
async fn a_test_restore_checks_the_copy_and_leaves_the_service_untouched() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (run_id, _, root_id) = backed_up(&harness, directory.path()).await;
    let layout = Layout::new(harness.database.path());
    let secrets_before = files_below(&directory.path().join("secrets"));
    let moved = directory.path().join("moved-root");

    let (status, report) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/test",
        json!({
            "source": { "run_id": run_id },
            "passphrase": PASSPHRASE,
            "mappings": [{ "storage_root_id": root_id, "path": moved.display().to_string() }],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["schema"]["migrated"], 0);
    assert!(
        report["restored_credentials"].as_u64() >= Some(1),
        "{report}"
    );
    let root = report["roots"]
        .as_array()
        .expect("roots")
        .iter()
        .find(|root| root["id"] == root_id.as_str())
        .expect("mapped root")
        .clone();
    assert_eq!(root["mapped_to"], moved.display().to_string());
    // The moved root does not exist yet: a warning, not an error.
    assert!(
        report["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .any(|problem| problem["code"] == "backup.restore_root_missing"
                && problem["severity"] == "warning")
    );

    assert_eq!(files_below(&layout.work()), 0, "the throwaway copy stayed");
    assert_eq!(
        files_below(&directory.path().join("secrets")),
        secrets_before
    );
    let (_, status_body) = common::get_json(&harness.router, "/api/v1/backups/restore").await;
    assert_eq!(status_body["state"], "none");
    assert_eq!(
        harness
            .database
            .list_proxy_profiles()
            .await
            .expect("proxies")
            .len(),
        1
    );

    // A mapping onto a relative folder is refused before anything is written.
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/test",
        json!({
            "source": { "run_id": run_id },
            "passphrase": PASSPHRASE,
            "mappings": [{ "storage_root_id": root_id, "path": "relative/folder" }],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "backup.restore_mapping_not_absolute");
    assert_eq!(files_below(&layout.work()), 0);
}

#[tokio::test]
async fn a_staged_restore_can_be_discarded_with_the_credentials_it_stored() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (run_id, _, _) = backed_up(&harness, directory.path()).await;
    let layout = Layout::new(harness.database.path());
    let request = json!({ "source": { "run_id": run_id }, "passphrase": PASSPHRASE });

    let (status, body) =
        common::post_json(&harness.router, "/api/v1/backups/restore", request.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["status"]["state"], "staged");
    let (status, again) =
        common::post_json(&harness.router, "/api/v1/backups/restore", request).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(again["code"], "backup.restore_pending_exists");

    // The staged copy's proxy points at a fresh reference holding the backed-up credential.
    let staged = layout.staged().join(rd_backup::manifest::DATABASE_PART);
    let cells =
        rd_db::restore_copy::read_cells(&staged, &[rd_db::restore_copy::BUNDLED_SECRET_COLUMNS[2]])
            .await
            .expect("staged proxies");
    let minted = cells[0][0].value.clone();
    let value = harness
        .secrets
        .get(&minted)
        .await
        .expect("minted credential");
    assert_eq!(secrecy::ExposeSecret::expose_secret(&value), CANARY);

    let (status, body) = common::delete_json(&harness.router, "/api/v1/backups/restore").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "none");
    assert!(!layout.staged().exists());
    assert!(
        harness.secrets.get(&minted).await.is_err(),
        "the credential stayed"
    );
}

#[tokio::test]
async fn a_staged_restore_becomes_the_installation_at_the_next_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (run_id, _, _) = backed_up(&harness, directory.path()).await;
    // Changed after the backup: the restore takes it back.
    harness
        .database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "After the backup".to_owned(),
            kind: rd_core::ProxyKind::Http,
            endpoint: "http://127.0.0.1:3128".parse().expect("proxy URL"),
            username: None,
            secret_ref: None,
        })
        .await
        .expect("second proxy");
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore",
        json!({ "source": { "run_id": run_id }, "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let layout = Layout::new(harness.database.path());
    // The running service still has its own state.
    assert_eq!(
        harness
            .database
            .list_proxy_profiles()
            .await
            .expect("proxies")
            .len(),
        2
    );

    // The next start: the switch, then the service over the restored database.
    // A start comes after the old process has let go of its files; Windows refuses to move a
    // database file another handle still holds.
    harness.database.close().await.expect("close the database");
    drop(harness);
    let outcome = cutover::apply_pending(&layout).expect("start");
    assert!(matches!(outcome, Cutover::Switched(_)), "{outcome:?}");
    let harness = common::test_harness(directory.path()).await;
    let proxies = harness
        .database
        .list_proxy_profiles()
        .await
        .expect("proxies");
    assert_eq!(proxies.len(), 1);
    let reference = proxies[0].secret_ref.clone().expect("credential");
    let value = harness.secrets.get(&reference).await.expect("credential");
    assert_eq!(secrecy::ExposeSecret::expose_secret(&value), CANARY);
    // The backup schedule kept its key: the archive was sealed under it.
    assert!(
        harness
            .database
            .backup_config()
            .await
            .expect("config")
            .key
            .is_some()
    );
    cutover::finish(&layout).expect("finish");
    assert!(!layout.previous().exists());
    let (_, status_body) = common::get_json(&harness.router, "/api/v1/backups/restore").await;
    assert_eq!(status_body["state"], "none");
}

#[tokio::test]
async fn an_archive_arrives_in_chunks_at_their_offsets() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (_, archive, _) = backed_up(&harness, directory.path()).await;
    let bytes = std::fs::read(&archive).expect("archive");
    let half = bytes.len() / 2;

    let (status, upload) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/uploads",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{upload}");
    let id = upload["id"].as_str().expect("id").to_owned();
    let chunk = |offset: usize, data: Vec<u8>| {
        common::request_to(
            "PUT",
            &format!("/api/v1/backups/restore/uploads/{id}?offset={offset}"),
        )
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(data))
        .expect("request")
    };
    let (status, body) = common::send(&harness.router, chunk(0, bytes[..half].to_vec())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["size"], half);
    // A repeated chunk is refused, not appended twice.
    let (status, body) = common::send(&harness.router, chunk(0, bytes[..half].to_vec())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "backup.restore_upload_offset");
    let (status, body) = common::send(&harness.router, chunk(half, bytes[half..].to_vec())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["size"], bytes.len());

    let (status, preview) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        json!({ "source": { "upload_id": id }, "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");

    let (status, _) = common::delete_json(
        &harness.router,
        &format!("/api/v1/backups/restore/uploads/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = common::send(&harness.router, chunk(0, vec![1])).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // An id that is not one of ours names no file.
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        json!({ "source": { "upload_id": "../../api-test.sqlite3" }, "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "backup.restore_upload_unknown");
    // At most four uploads wait at once.
    for _ in 0..4 {
        let (status, body) = common::post_json(
            &harness.router,
            "/api/v1/backups/restore/uploads",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/uploads",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "backup.restore_uploads_full");
    // Two sources at once are refused.
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        json!({
            "source": { "upload_id": id, "path": archive.display().to_string() },
            "passphrase": PASSPHRASE,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "backup.restore_source_invalid");
}

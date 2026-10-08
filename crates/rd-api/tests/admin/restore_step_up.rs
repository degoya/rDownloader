//! A restore and a settings import take a signed-in session and the password, and a restore
//! checks the archive it is given (RD-1190-19).
//!
//! Both replace the password hash, the passkeys and the tokens with what the archive or the
//! bundle holds; with `api:admin` alone a token could restore an archive of its own and so
//! replace the way in. A restore from a recorded run compares the run's digest, so an older
//! archive put under its name is refused, and a damaged database copy is refused before it is
//! migrated.

use std::path::Path;
use std::time::Duration;

use axum::http::StatusCode;
use rd_backup::archive::{digest_file, extract_archive, write_archive};
use rd_backup::manifest::DATABASE_PART;
use rd_backup::restore::cutover::{self, Layout};
use serde_json::json;

use crate::common::{self, Harness, Options};
use crate::full_restore::{PASSPHRASE, backed_up};

const PASSWORD: &str = "restore-step-up-password";

#[tokio::test]
async fn a_restore_and_an_import_take_a_session_and_the_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(directory.path(), Options::default().login()).await;
    let router = &harness.router;
    let session = common::sign_in(router, PASSWORD).await;
    let (status, bundle) = common::post_json_with_cookie(
        router,
        "/api/v1/settings/export",
        &session,
        json!({ "include_secrets": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bundle}");
    let restore = |password: Option<&str>| {
        json!({
            "source": { "run_id": "no-such-run" },
            "passphrase": PASSPHRASE,
            "password": password,
        })
    };
    let import = |password: Option<&str>| json!({ "bundle": bundle, "password": password });

    // A token holding every area, the password included, is no signed-in session.
    for (uri, body) in [
        ("/api/v1/backups/restore", restore(Some(PASSWORD))),
        ("/api/v1/settings/import", import(Some(PASSWORD))),
    ] {
        let (status, body) = common::post_with_bearer(router, uri, common::API_BEARER, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(body["code"], "auth.step_up_session_required", "{uri}");
    }

    // A session without the password, or with a wrong one, is refused as a wrong sign-in (two
    // failures: the limiter locks an address out after five).
    for (uri, body) in [
        ("/api/v1/backups/restore", restore(None)),
        ("/api/v1/settings/import", import(Some("not-the-password"))),
    ] {
        let (status, body) = common::post_json_with_cookie(router, uri, &session, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}: {body}");
        assert_eq!(body["code"], "auth.invalid_credentials", "{uri}");
    }

    // With the password the restore reaches its own checks, and the import happens.
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/backups/restore",
        &session,
        restore(Some(PASSWORD)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "backup.restore_run_unknown");
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/settings/import",
        &session,
        import(Some(PASSWORD)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// One more run of the backup `backed_up` set up; returns its archive's name.
async fn another_run(harness: &Harness) -> String {
    let (status, run) = common::post_json(&harness.router, "/api/v1/backups/runs", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    let id = run["id"].as_str().expect("run id").to_owned();
    let run = common::eventually(
        Duration::from_secs(60),
        "the second backup run to finish",
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
    assert_eq!(run.state, rd_core::BackupRunState::Succeeded);
    run.archive_name.expect("archive")
}

#[tokio::test]
async fn an_older_archive_under_a_runs_name_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (first_run, first_archive, _) = backed_up(&harness, directory.path()).await;
    // Archive names carry the second the run started in.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second_name = another_run(&harness).await;
    let second_run = harness
        .database
        .backup_runs(10)
        .await
        .expect("runs")
        .into_iter()
        .find(|run| run.archive_name.as_deref() == Some(second_name.as_str()))
        .expect("the second run")
        .id;

    // Whoever writes the destination rolls the second run back to the first archive: the same
    // key opens it, only the recorded digest tells them apart.
    let second_archive = first_archive.with_file_name(&second_name);
    std::fs::copy(&first_archive, &second_archive).expect("roll back");
    let preview = |run: &str| json!({ "source": { "run_id": run }, "passphrase": PASSPHRASE });
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        preview(&second_run),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "backup.restore_archive_changed");

    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        preview(&first_run),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// `archive` sealed again under its own key into `out`, after `change` had its database copy.
async fn resealed(archive: &Path, out: &Path, change: impl FnOnce(&Path)) {
    let key = rd_backup::restore::inspect::key_for(archive, PASSPHRASE)
        .await
        .expect("the passphrase opens the archive");
    let folder = out.with_extension("unpacked");
    std::fs::create_dir_all(&folder).expect("folder");
    let mut manifest = extract_archive(archive, &key, &folder).expect("unpack");
    let database = folder.join(DATABASE_PART);
    change(&database);
    let (size, sha256) = digest_file(&database).expect("digest");
    for part in manifest
        .parts
        .iter_mut()
        .filter(|part| part.name == DATABASE_PART)
    {
        part.size = size;
        part.sha256.clone_from(&sha256);
    }
    write_archive(&folder, &manifest, &key, out).expect("seal");
}

#[tokio::test]
async fn a_damaged_database_copy_is_refused_before_anything_is_staged() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (_, archive, _) = backed_up(&harness, directory.path()).await;
    let damaged = directory.path().join("damaged.rdbackup");
    // The header's freelist count raised by one: every page still reads, a write that needs a
    // page fails, and `PRAGMA integrity_check` names it.
    resealed(&archive, &damaged, |database| {
        let mut bytes = std::fs::read(database).expect("read copy");
        let count = u32::from_be_bytes(bytes[36..40].try_into().expect("four bytes"));
        bytes[36..40].copy_from_slice(&(count + 1).to_be_bytes());
        std::fs::write(database, bytes).expect("write copy");
    })
    .await;

    let request = json!({
        "source": { "path": damaged.display().to_string() },
        "passphrase": PASSPHRASE,
        "mappings": [],
    });
    for uri in ["/api/v1/backups/restore/test", "/api/v1/backups/restore"] {
        let (status, body) = common::post_json(&harness.router, uri, request.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}: {body}");
        assert_eq!(body["code"], "backup.restore_database_damaged", "{uri}");
    }
    let layout = Layout::new(harness.database.path());
    assert!(cutover::read_marker(&layout).expect("marker").is_none());
}

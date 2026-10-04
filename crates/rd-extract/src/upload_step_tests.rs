//! Audit 1.9.1, INTAKE-01: an upload step through a plugin destination closes the row it was
//! planned under, so no `Queued` row is left behind and a restart does not run the pipeline
//! again.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{PackageId, PackageState, PostprocessKind, PostprocessState};
use rd_db::{Database, NewPackage};

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger, StorageUpload, StorageUploader,
    UploadReport,
    tests::{seed_completed_file, zip_bytes},
};

const REMOTE: &str = "plugin:counting/https://dav.example/in";

/// A destination that confirms everything and counts how often it was asked.
#[derive(Default)]
struct CountingDestination {
    uploads: AtomicUsize,
}

#[async_trait]
impl StorageUploader for CountingDestination {
    fn installed(&self, _plugin_id: &str) -> bool {
        true
    }

    async fn upload(&self, _plugin_id: &str, upload: StorageUpload<'_>) -> Result<UploadReport> {
        self.uploads.fetch_add(1, Ordering::SeqCst);
        Ok(UploadReport::Verified {
            files: upload.files.to_vec(),
        })
    }
}

fn config(temp: &std::path::Path) -> ExtractionConfig {
    ExtractionConfig {
        default_passwords_file: temp.join("passwords.txt"),
        rar_timeout: Duration::from_secs(5),
        default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
        hold: rd_core::PostprocessHold::new(),
        quiet_hold: rd_core::PostprocessHold::new(),
        upload_limit: None,
    }
}

/// A database with one finished package whose settings upload it to [`REMOTE`].
async fn package_to_upload(temp: &std::path::Path) -> (Database, PackageId) {
    package_to_upload_with(temp, None).await
}

/// The same, with a user script from `temp/scripts` when `script` names one.
async fn package_to_upload_with(
    temp: &std::path::Path,
    script: Option<&str>,
) -> (Database, PackageId) {
    let database = Database::open(temp.join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({
                "upload_enabled": true,
                "upload_remote": REMOTE,
                "upload_mode": "copy",
                "scripts_directory": temp.join("scripts").to_string_lossy(),
            }),
        )
        .await
        .expect("settings");
    let destination = temp.join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    std::fs::write(destination.join("Film.zip"), zip_bytes("film.mkv", b"film")).expect("zip");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "uploaded".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: script.map(str::to_owned),
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    seed_completed_file(&database, package.id, "Film.zip").await;
    (database, package.id)
}

async fn settled(database: &Database, package: PackageId) -> PackageState {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = database
                .get_package(package)
                .await
                .expect("package")
                .expect("package exists")
                .state;
            if matches!(state, PackageState::Completed | PackageState::Failed) {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("post-processing finished")
}

async fn upload_rows(database: &Database, package: PackageId) -> Vec<rd_core::PostprocessStep> {
    database
        .list_postprocess_steps(&package.to_string())
        .await
        .expect("steps")
        .into_iter()
        .filter(|step| step.kind == PostprocessKind::Upload)
        .collect()
}

#[tokio::test]
async fn a_plugin_upload_closes_its_planned_row_and_a_restart_does_not_run_it_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, package) = package_to_upload(temp.path()).await;

    let destination = Arc::new(CountingDestination::default());
    let service = ExtractionService::start_with_plugins(
        database.clone(),
        config(temp.path()),
        None,
        Some(destination.clone() as Arc<dyn StorageUploader>),
        None,
    );
    service
        .request(package, ExtractionTrigger::Manual)
        .await
        .expect("request");
    assert_eq!(settled(&database, package).await, PackageState::Completed);
    service.shutdown().await;

    // One row, the planned one, and it is done: before the fix the checkpoints went to a
    // second row named after the destination and the planned one stayed queued.
    let rows = upload_rows(&database, package).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].source_path, REMOTE);
    assert_eq!(rows[0].state, PostprocessState::Completed);
    assert_eq!(
        rows[0].code.as_deref(),
        Some(crate::upload_step::UPLOAD_VERIFIED)
    );
    assert!(rows[0].params.contains_key("count"), "{rows:?}");
    let steps = database
        .list_postprocess_steps(&package.to_string())
        .await
        .expect("steps");
    assert!(
        steps
            .iter()
            .all(|step| step.state != PostprocessState::Queued),
        "{steps:?}"
    );
    assert_eq!(destination.uploads.load(Ordering::SeqCst), 1);

    // A restart: nothing is queued, so recovery finds nothing to run again.
    let restarted = Arc::new(CountingDestination::default());
    let service = ExtractionService::start_with_plugins(
        database.clone(),
        config(temp.path()),
        None,
        Some(restarted.clone() as Arc<dyn StorageUploader>),
        None,
    );
    service.recover().await.expect("recover");
    tokio::time::sleep(Duration::from_millis(300)).await;
    service.shutdown().await;
    assert_eq!(restarted.uploads.load(Ordering::SeqCst), 0);
    assert_eq!(
        database
            .get_package(package)
            .await
            .expect("package")
            .expect("package exists")
            .state,
        PackageState::Completed
    );
}

#[tokio::test]
async fn a_destination_that_is_gone_fails_its_planned_row_with_a_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, package) = package_to_upload(temp.path()).await;
    // The upload is planned from the settings whether or not a destination is loaded; the run
    // then finds none and has to say so on the planned row, not on a second one.
    let service = ExtractionService::start(database.clone(), config(temp.path()));
    service
        .request(package, ExtractionTrigger::Manual)
        .await
        .expect("request");
    assert_eq!(settled(&database, package).await, PackageState::Failed);
    service.shutdown().await;

    let rows = upload_rows(&database, package).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].source_path, REMOTE);
    assert_eq!(rows[0].state, PostprocessState::Failed);
    assert_eq!(
        rows[0].code.as_deref(),
        Some(crate::upload_step::UPLOAD_UNAVAILABLE)
    );
}

#[cfg(unix)]
/// A destination whose upload a service stop interrupted, as the plugin host reports one.
struct StoppedDestination;

#[async_trait]
#[cfg(unix)]
impl StorageUploader for StoppedDestination {
    fn installed(&self, _plugin_id: &str) -> bool {
        true
    }

    async fn upload(&self, _plugin_id: &str, _upload: StorageUpload<'_>) -> Result<UploadReport> {
        Ok(UploadReport::Stopped)
    }
}

#[cfg(unix)]
/// Waits until the service has no package queued or running.
async fn idle(service: &ExtractionService) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !service.pending().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("post-processing finished");
}

#[cfg(unix)]
async fn package_state(database: &Database, package: PackageId) -> PackageState {
    database
        .get_package(package)
        .await
        .expect("package")
        .expect("package exists")
        .state
}

/// Audit 1.9.1, RA-IN-01 (and RA-IN-07): a stop during the upload is no verdict. The package stays in
/// post-processing with its upload queued instead of being marked completed, and the restart
/// resumes it: the upload runs, the user script that already ran does not run a second time.
#[cfg(unix)]
#[tokio::test]
async fn a_stop_during_the_upload_resumes_after_a_restart_without_running_the_script_again() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir().expect("tempdir");
    let (database, package) = package_to_upload_with(temp.path(), Some("count.sh")).await;
    let scripts = temp.path().join("scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    let runs = temp.path().join("runs.txt");
    let script = scripts.join("count.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho ran >> '{}'\n", runs.display()),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let script_runs = || {
        std::fs::read_to_string(&runs)
            .unwrap_or_default()
            .lines()
            .count()
    };

    let service = ExtractionService::start_with_plugins(
        database.clone(),
        config(temp.path()),
        None,
        Some(Arc::new(StoppedDestination) as Arc<dyn StorageUploader>),
        None,
    );
    service
        .request(package, ExtractionTrigger::Manual)
        .await
        .expect("request");
    idle(&service).await;
    service.shutdown().await;
    assert_eq!(script_runs(), 1);
    assert_eq!(
        package_state(&database, package).await,
        PackageState::Postprocessing
    );
    let rows = upload_rows(&database, package).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].state, PostprocessState::Queued);
    // RA-IN-07: the queue shows where the upload goes, not the plugin id in front of it.
    let shown = database
        .get_package(package)
        .await
        .expect("package")
        .expect("package exists")
        .postprocess
        .current;
    assert_eq!(shown.as_deref(), Some("https://dav.example/in"));

    // The restart resumes the package and finishes it.
    let restarted = Arc::new(CountingDestination::default());
    let service = ExtractionService::start_with_plugins(
        database.clone(),
        config(temp.path()),
        None,
        Some(restarted.clone() as Arc<dyn StorageUploader>),
        None,
    );
    service.recover().await.expect("recover");
    assert_eq!(settled(&database, package).await, PackageState::Completed);
    service.shutdown().await;
    assert_eq!(restarted.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(script_runs(), 1, "the script ran a second time");
    let rows = upload_rows(&database, package).await;
    assert_eq!(rows[0].state, PostprocessState::Completed);
}

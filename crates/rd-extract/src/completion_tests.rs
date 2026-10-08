//! Audit 1.9.1, INTAKE-04: a completion the event bus dropped (a lagged receiver) is found by
//! the sweep the lag triggers, so the package is post-processed all the same. RA-IN-05: one
//! package the sweep cannot request does not end it.

use std::time::Duration;

use rd_core::{PackageId, PackageState};
use rd_db::{Database, NewPackage};

use crate::{ExtractionConfig, ExtractionService, tests::seed_completed_file};

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

async fn settled_package(database: &Database, temp: &std::path::Path, name: &str) -> PackageId {
    let destination = temp.join(name);
    std::fs::create_dir_all(&destination).expect("destination");
    std::fs::write(destination.join("payload.bin"), b"payload").expect("file");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    seed_completed_file(database, package.id, "payload.bin").await;
    package.id
}

#[tokio::test]
async fn a_package_the_sweep_cannot_request_does_not_end_the_sweep() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let first = settled_package(&database, temp.path(), "first").await;
    settled_package(&database, temp.path(), "second").await;
    let service = ExtractionService::start(database.clone(), config(temp.path()));
    // With the job loop gone every request is refused, the first package's included.
    service.shutdown().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while service
            .request(first, crate::ExtractionTrigger::Manual)
            .await
            .is_ok()
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the job loop ended");
    // Before the fix the first refusal ended the sweep with an error.
    service
        .sweep_settled_packages()
        .await
        .expect("the sweep goes on past a package it cannot request");
}

#[tokio::test]
async fn the_sweep_after_a_lag_post_processes_a_package_whose_completion_was_missed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    std::fs::write(destination.join("payload.bin"), b"payload").expect("file");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "missed".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    // Completed before the service subscribes: its events are gone, exactly as the ones a
    // lagged receiver skips are.
    seed_completed_file(&database, package.id, "payload.bin").await;

    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let state = |database: Database| async move {
        database
            .get_package(package.id)
            .await
            .expect("package")
            .expect("package exists")
            .state
    };
    assert!(
        !matches!(
            state(database.clone()).await,
            PackageState::Completed | PackageState::Failed
        ),
        "nothing asked for the package yet"
    );

    service.sweep_settled_packages().await.expect("sweep");
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current = state(database.clone()).await;
            if matches!(current, PackageState::Completed | PackageState::Failed) {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the sweep requested the package");
    service.shutdown().await;
    assert_eq!(settled, PackageState::Completed);
}

/// RD-1190-13: a file of the package that is blocked — the account the owner's DDownload limit
/// was mistaken for an invalid one — is a part that is missing. Until 1.19 a blocked file counted
/// as settled and the package was unpacked around it; now it is not post-processed, and the
/// file completing after a retry is what starts the pipeline.
#[tokio::test]
async fn a_blocked_file_holds_post_processing_back_until_it_completes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let package = settled_package(&database, temp.path(), "waiting").await;
    std::fs::write(temp.path().join("waiting").join("second.bin"), b"second").expect("file");
    let second = database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package,
            source: "https://example.test/second.bin".parse().expect("URL"),
            file_name: "second.bin".to_owned(),
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
        .record_failure(
            second.id,
            rd_core::Failure::coded(
                rd_core::FailureKind::AccountInvalid,
                "ddownload.no_premium_file",
                "no premium file",
            ),
            None,
        )
        .await
        .expect("the file is blocked");
    let service = ExtractionService::start(database.clone(), config(temp.path()));
    let state = |database: Database| async move {
        database
            .get_package(package)
            .await
            .expect("package")
            .expect("package exists")
            .state
    };

    service.sweep_settled_packages().await.expect("sweep");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !matches!(
            state(database.clone()).await,
            PackageState::Postprocessing | PackageState::Completed | PackageState::Failed
        ),
        "post-processed around a blocked file"
    );

    for next in [
        rd_core::DownloadState::Queued,
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        database
            .transition_download(second.id, next)
            .await
            .expect("transition");
    }
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current = state(database.clone()).await;
            if matches!(current, PackageState::Completed | PackageState::Failed) {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the last file's completion started the pipeline");
    service.shutdown().await;
    assert_eq!(settled, PackageState::Completed);
}

/// RD-1190-13: which files hold a package's post-processing back, and which package counts as
/// missing a part. Usenet keeps starting over a failed file, which PAR2 may rebuild.
#[tokio::test]
async fn only_completed_or_stood_down_files_let_a_hoster_package_start() {
    use rd_core::{DownloadKind, DownloadState};

    use crate::completion::{parts_missing, ready_for_postprocess};

    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let package = settled_package(&database, temp.path(), "gate").await;
    let completed = database
        .downloads_for_package(package)
        .await
        .expect("files")
        .into_iter()
        .next()
        .expect("one file");
    let with = |state: DownloadState| {
        let mut other = completed.clone();
        other.state = state;
        vec![completed.clone(), other]
    };

    for state in [
        DownloadState::Completed,
        DownloadState::Seeding,
        DownloadState::Skipped,
        DownloadState::Cancelled,
    ] {
        assert!(
            ready_for_postprocess(DownloadKind::Http, &with(state)),
            "{state:?}"
        );
        assert!(
            !parts_missing(DownloadKind::Http, &with(state)),
            "{state:?}"
        );
    }
    for state in [
        DownloadState::RetryWait,
        DownloadState::Queued,
        DownloadState::Paused,
        DownloadState::Failed,
        DownloadState::Blocked,
    ] {
        assert!(
            !ready_for_postprocess(DownloadKind::Http, &with(state)),
            "{state:?}"
        );
        assert!(parts_missing(DownloadKind::Http, &with(state)), "{state:?}");
    }
    assert!(ready_for_postprocess(
        DownloadKind::Usenet,
        &with(DownloadState::Failed)
    ));
    assert!(!parts_missing(
        DownloadKind::Usenet,
        &with(DownloadState::Failed)
    ));
    assert!(!ready_for_postprocess(
        DownloadKind::Usenet,
        &with(DownloadState::RetryWait)
    ));
    let mut nothing_finished = with(DownloadState::Skipped);
    nothing_finished[0].state = DownloadState::Cancelled;
    assert!(
        !ready_for_postprocess(DownloadKind::Http, &nothing_finished),
        "at least one file has to have completed"
    );
}

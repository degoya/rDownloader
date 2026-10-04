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

//! Crash and restart of dissolving a folder named like the package (RD-1140-01, recovery
//! matrix).
//!
//! The same shape as `sort_crash_tests`: the crash point returns an error out of `run_package`
//! without the job loop around it, which leaves what a killed process leaves — one entry moved
//! up, the rest in the working folder, the package `Postprocessing` — and the restart is a new
//! service whose `recover` finds it.

use std::time::Duration;

use rd_core::PackageState;

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger,
    tests::{extraction_inner, wait_until_finished},
    unwrap_job::STAGING,
    unwrap_job_tests::{database_with, seed},
};

/// `postprocess.after_unwrap_move`: one entry is up, the working folder holds the rest.
#[tokio::test]
async fn a_dissolve_stopped_between_two_moves_is_finished_by_the_next_start() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "default_level": "delete", "unwrap_package_folder": true }),
    )
    .await;
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Release",
        None,
        &[
            ("Release/a.mkv", b"first".to_vec()),
            ("Release/b.mkv", b"second".to_vec()),
        ],
    )
    .await;

    {
        let guard = rd_core::failpoint::FailpointGuard::once("postprocess.after_unwrap_move");
        let inner = extraction_inner(&database, temp.path());
        let result =
            crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto).await;
        assert!(result.is_err(), "the pipeline ran past its crash point");
        assert!(guard.fired(), "the crash point was never reached");
    }
    // The entries move in name order: `a.mkv` is up, `b.mkv` waits in the working folder.
    assert!(directory.join("a.mkv").is_file());
    assert!(directory.join(STAGING).join("b.mkv").is_file());
    assert!(!directory.join("Release").exists());

    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: temp.path().join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    service.recover().await.expect("recover");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let package = database
                .get_package(package_id)
                .await
                .expect("package")
                .expect("package row");
            if package.state != PackageState::Postprocessing {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the restart finished the pipeline");
    wait_until_finished(&service, package_id).await;
    service.shutdown().await;

    assert_eq!(
        std::fs::read(directory.join("a.mkv")).ok(),
        Some(b"first".to_vec())
    );
    assert_eq!(
        std::fs::read(directory.join("b.mkv")).ok(),
        Some(b"second".to_vec())
    );
    assert!(
        !directory.join(STAGING).exists(),
        "the working folder lingers"
    );
    assert_eq!(std::fs::read_dir(&directory).expect("package").count(), 2);
    let package = database
        .get_package(package_id)
        .await
        .expect("package")
        .expect("package row");
    assert_eq!(package.state, PackageState::Completed);
}

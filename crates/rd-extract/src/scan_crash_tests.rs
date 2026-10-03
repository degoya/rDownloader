//! Crash and restart of the malware scan (RD-190-14, recovery matrix).
//!
//! The same shape as `crash_tests`: the crash point returns an error out of `run_package`
//! without the job loop around it, which leaves what a killed process leaves — the scan step
//! `Running`, the package `Postprocessing` — and the restart is a new service whose `recover`
//! finds it.

use std::time::Duration;

use rd_core::{PackageId, PackageState, PostprocessKind, PostprocessState};
use rd_db::Database;

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger,
    fake_clamd::{FakeClamd, eicar},
    malware_scan,
    malware_scan_tests::{scan_settings, seed_package},
    tests::{extraction_inner, wait_until_finished},
};

async fn package_state(database: &Database, package_id: PackageId) -> PackageState {
    database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package")
        .state
}

/// `postprocess.before_scan_recorded`: clamd answered, the step does not say so.
#[tokio::test]
async fn a_scan_stopped_before_its_verdict_was_recorded_is_scanned_again_and_never_released() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let clamd = FakeClamd::start().await;
    scan_settings(
        &database,
        serde_json::json!({
            "clamd_address": clamd.address,
            "upload_enabled": true,
            "upload_remote": "archive:releases",
        }),
    )
    .await;
    let destination = temp.path().join("dl");
    let package_id = seed_package(&database, &destination, "eicar.com", &eicar()).await;

    {
        let guard = rd_core::failpoint::FailpointGuard::once("postprocess.before_scan_recorded");
        let inner = extraction_inner(&database, temp.path());
        let result =
            crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto).await;
        assert!(result.is_err(), "the pipeline ran past its crash point");
        assert!(guard.fired(), "the crash point was never reached");
    }
    let first_pass = clamd.scans();
    assert!(first_pass >= 1, "the first pass scanned nothing");
    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let scan = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::MalwareScan)
        .expect("scan step");
    assert_eq!(scan.state, PostprocessState::Running, "{scan:?}");
    assert_eq!(
        package_state(&database, package_id).await,
        PackageState::Postprocessing
    );

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
    let mut states = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = package_state(&database, package_id).await;
            states.push(state);
            if state != PackageState::Postprocessing {
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
        package_state(&database, package_id).await,
        PackageState::Failed
    );
    assert!(
        !states.contains(&PackageState::Completed),
        "the package was released: {states:?}"
    );
    assert!(clamd.scans() > first_pass, "the restart did not scan again");
    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let scan = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::MalwareScan)
        .expect("scan step");
    assert_eq!(scan.state, PostprocessState::Failed, "{scan:?}");
    assert_eq!(scan.code.as_deref(), Some(malware_scan::FOUND));
    let upload = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Upload)
        .expect("upload step");
    assert_eq!(upload.state, PostprocessState::Skipped, "{upload:?}");
}

//! Crash and restart of the post-processing pipeline (RD-180-12, recovery matrix).
//!
//! The crash point returns an error out of `run_package` without the job loop around it, so
//! nothing records the package as failed: what is left is exactly what a killed process leaves,
//! a package still `Postprocessing` and a step still `Running`. The restart is a new service
//! over the same database whose `recover` finds it, which is what every start runs.

use std::time::Duration;

use rd_core::{PackageId, PackageState, PostprocessKind, PostprocessState};
use rd_db::Database;

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger,
    tests::{extraction_inner, seed_nested_package, wait_until_finished},
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

async fn unpack_steps(database: &Database, package_id: PackageId) -> Vec<rd_core::PostprocessStep> {
    database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps")
        .into_iter()
        .filter(|step| step.kind == PostprocessKind::ExtractZip)
        .collect()
}

fn staging_left(directory: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .expect("package folder")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(rd_postprocess::STAGING_PREFIX))
        .collect()
}

/// `postprocess.before_unpack_recorded`: the archive is unpacked, the step does not say so.
///
/// The restart also finds the staging directory an extraction killed half way leaves behind
/// (Axis B's case, planted here by hand): the rerun removes it rather than leaving partial
/// output in the package folder for good.
#[tokio::test]
async fn an_unpack_stopped_before_its_step_was_recorded_is_unpacked_again_by_the_next_start() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    let package_id = seed_nested_package(&database, &destination, 1).await;

    {
        let guard = rd_core::failpoint::FailpointGuard::once("postprocess.before_unpack_recorded");
        let inner = extraction_inner(&database, temp.path());
        let result =
            crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto).await;
        assert!(result.is_err(), "the pipeline ran past its crash point");
        assert!(guard.fired(), "the crash point was never reached");
    }
    // What the stop left: the payload on disk, a step that never finished, a package that
    // still says it is being post-processed.
    assert_eq!(
        std::fs::read(destination.join("payload.txt")).ok(),
        Some(b"payload".to_vec())
    );
    let steps = unpack_steps(&database, package_id).await;
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert_eq!(steps[0].state, PostprocessState::Running);
    assert_eq!(
        package_state(&database, package_id).await,
        PackageState::Postprocessing
    );
    let killed = destination.join(format!("{}killed", rd_postprocess::STAGING_PREFIX));
    std::fs::create_dir_all(&killed).expect("staging");
    std::fs::write(killed.join("payload.txt"), b"pay").expect("partial output");

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
        while package_state(&database, package_id).await == PackageState::Postprocessing {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the restart finished the pipeline");
    wait_until_finished(&service, package_id).await;
    service.shutdown().await;

    assert_eq!(
        package_state(&database, package_id).await,
        PackageState::Completed
    );
    // The same bytes an uninterrupted run produces, one step for the set, nothing staged.
    assert_eq!(
        std::fs::read(destination.join("payload.txt")).ok(),
        Some(b"payload".to_vec())
    );
    let steps = unpack_steps(&database, package_id).await;
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert_eq!(steps[0].state, PostprocessState::Completed);
    assert!(
        staging_left(&destination).is_empty(),
        "{:?}",
        staging_left(&destination)
    );
}

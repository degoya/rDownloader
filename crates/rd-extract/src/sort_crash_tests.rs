//! Crash and restart of the sort (RD-1100-08, recovery matrix).
//!
//! The same shape as `scan_crash_tests`: the crash point returns an error out of `run_package`
//! without the job loop around it, which leaves what a killed process leaves — one file placed,
//! the rest in the package, the step `Running`, the package `Postprocessing` — and the restart
//! is a new service whose `recover` finds it.

use std::time::Duration;

use rd_core::{PackageState, PostprocessState};

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger,
    sort_job_tests::{prepared, sort_step, sorted_package},
    tests::{extraction_inner, wait_until_finished},
};

/// `postprocess.after_sort_move`: a file is placed, the step does not say so.
#[tokio::test]
async fn a_sort_stopped_between_two_moves_places_the_rest_on_the_next_start() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, category_id, library) = prepared(temp.path()).await;
    let name = "Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE";
    let video = format!("{name}.mkv");
    let subtitle = format!("{name}.en.srt");
    let nfo = format!("{name}.nfo");
    let (package_id, directory) = sorted_package(
        &database,
        &library,
        category_id,
        name,
        &[video.as_str()],
        &[subtitle.as_str(), nfo.as_str()],
    )
    .await;
    let season = library.join("Breaking Bad").join("Season 05");
    let placed = |suffix: &str| season.join(format!("Breaking Bad - S05E14 - Ozymandias{suffix}"));

    {
        let guard = rd_core::failpoint::FailpointGuard::once("postprocess.after_sort_move");
        let inner = extraction_inner(&database, temp.path());
        let result =
            crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto).await;
        assert!(result.is_err(), "the pipeline ran past its crash point");
        assert!(guard.fired(), "the crash point was never reached");
    }
    // One companion went first; the video, which the next start recognises the rest by, stayed.
    assert!(directory.join(&video).is_file());
    let first = [".en.srt", ".nfo"]
        .iter()
        .filter(|suffix| placed(suffix.to_owned()).is_file())
        .count();
    assert_eq!(first, 1, "the first pass placed {first} files");
    assert_eq!(
        sort_step(&database, package_id).await.state,
        PostprocessState::Running
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

    for suffix in [".mkv", ".en.srt", ".nfo"] {
        assert!(placed(suffix).is_file(), "{suffix} did not land");
    }
    // Nothing placed twice, nothing beside it under a second name.
    assert_eq!(std::fs::read_dir(&season).expect("season").count(), 3);
    assert!(!directory.exists(), "the emptied package folder lingers");
    let package = database
        .get_package(package_id)
        .await
        .expect("package")
        .expect("package row");
    assert_eq!(package.state, PackageState::Completed);
    assert_eq!(
        sort_step(&database, package_id).await.state,
        PostprocessState::Completed
    );
}

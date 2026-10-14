//! A category move during post-processing (RD-1290-01): held back while the pipeline owns the
//! folder, carried over by itself once the package is completed or failed.

use std::path::Path;
use std::time::Duration;

use rd_core::{DownloadFile, DownloadState, PackageState};

use super::super::tests::paused_package;
use super::postprocess_ended;
use crate::SchedulerHandle;

/// Completes the paused file the way a finished transfer does.
async fn complete(scheduler: &SchedulerHandle, file: &DownloadFile) {
    for state in [
        DownloadState::Downloading,
        DownloadState::Verifying,
        DownloadState::Completed,
    ] {
        scheduler
            .database
            .transition_download(file.id, state)
            .await
            .expect("transition");
    }
}

async fn set_state(scheduler: &SchedulerHandle, file: &DownloadFile, state: PackageState) {
    let stage =
        (state == PackageState::Postprocessing).then_some(rd_core::PostprocessStage::Extracting);
    scheduler
        .database
        .set_package_state(file.package_id, state, stage, None, None)
        .await
        .expect("package state");
}

/// Points the package at `destination` the way a category change does.
async fn move_to(scheduler: &SchedulerHandle, file: &DownloadFile, destination: &Path) {
    scheduler
        .database
        .update_packages(
            vec![file.package_id],
            rd_db::PackageChange {
                category: Some(rd_db::CategoryAssignment {
                    category_id: None,
                    destinations: std::collections::HashMap::from([(
                        file.package_id,
                        destination.to_string_lossy().into_owned(),
                    )]),
                }),
                ..Default::default()
            },
        )
        .await
        .expect("category change");
    scheduler
        .relocate_package(file.package_id)
        .await
        .expect("relocate");
}

/// Asserts the move is still outstanding and nothing left the old folder.
async fn assert_held_back(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    old: &Path,
    new: &Path,
) {
    assert!(
        old.join(&file.file_name).exists(),
        "the archive left mid-pipeline"
    );
    assert!(
        !new.exists(),
        "the new folder appeared before the pipeline ended"
    );
    assert!(
        scheduler
            .database
            .package_previous_destination(file.package_id)
            .await
            .expect("previous destination")
            .is_some(),
        "the held-back move was forgotten"
    );
}

/// Waits until the package's data is in `new` and the move is no longer outstanding.
async fn await_moved(scheduler: &SchedulerHandle, file: &DownloadFile, new: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let outstanding = scheduler
            .database
            .package_previous_destination(file.package_id)
            .await
            .expect("previous destination");
        if outstanding.is_none() && new.join(&file.file_name).exists() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the move never followed the end of post-processing"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_move_waits_for_post_processing_and_follows_when_it_ends() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, old) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&old).await.expect("old folder");
    tokio::fs::write(old.join(&file.file_name), b"archive")
        .await
        .expect("archive");
    complete(&scheduler, &file).await;
    set_state(&scheduler, &file, PackageState::Postprocessing).await;

    let new = temporary.path().join("updates").join("Example Package");
    move_to(&scheduler, &file, &new).await;
    assert_held_back(&scheduler, &file, &old, &new).await;

    // What the unpack writes before it ends, then the end.
    tokio::fs::write(old.join("unpacked.bin"), b"output")
        .await
        .expect("output");
    set_state(&scheduler, &file, PackageState::Completed).await;

    await_moved(&scheduler, &file, &new).await;
    assert_eq!(
        tokio::fs::read(new.join("unpacked.bin"))
            .await
            .expect("unpacked output"),
        b"output",
        "the unpacked output did not follow"
    );
    assert!(!old.exists(), "the old package folder stayed behind");
}

/// The last file settled and the pipeline has not written its state yet: the folder is already
/// the pipeline's, so the move waits; a failed pipeline ends it as well as a finished one.
#[tokio::test]
async fn a_settled_package_waits_for_its_pipeline_to_start_and_end() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, old) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&old).await.expect("old folder");
    tokio::fs::write(old.join(&file.file_name), b"archive")
        .await
        .expect("archive");
    complete(&scheduler, &file).await;

    let new = temporary.path().join("updates").join("Example Package");
    move_to(&scheduler, &file, &new).await;
    assert_held_back(&scheduler, &file, &old, &new).await;

    set_state(&scheduler, &file, PackageState::Postprocessing).await;
    set_state(&scheduler, &file, PackageState::Failed).await;

    await_moved(&scheduler, &file, &new).await;
    assert!(!old.exists(), "the old package folder stayed behind");
}

#[test]
fn only_a_new_outcome_ends_post_processing() {
    let package = rd_core::PackageId::new();
    let event = |state: &str, previous: &str| {
        rd_core::EventEnvelope::new(
            rd_core::EventKind::PackageState,
            serde_json::json!({ "package_id": package, "state": state, "previous": previous }),
        )
    };
    assert_eq!(
        postprocess_ended(&event("completed", "postprocessing")),
        Some(package)
    );
    assert_eq!(
        postprocess_ended(&event("failed", "downloading")),
        Some(package)
    );
    assert_eq!(postprocess_ended(&event("completed", "completed")), None);
    assert_eq!(
        postprocess_ended(&event("postprocessing", "downloading")),
        None
    );
}

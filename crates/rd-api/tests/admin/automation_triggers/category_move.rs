//! `set_category` moves the package into its new category, end to end (RD-1290-01).
//!
//! The action wrote the row and never moved anything: the owner's "…Update…NSW…" packages kept
//! their files in the old folder while the queue named the hidden category. Now the data follows
//! -- at once for a finished package, after the pipeline for one still being post-processed --
//! and an archive volume's return from unpacking is no second `download_completed`.

use super::{common_download, intake, run_count, wait_for_runs_of};
use crate::common;

use axum::http::StatusCode;
use common::{WAIT, eventually, get_json, parked_harness, post_json};
use rd_core::{DownloadState, PackageId, PackageState};
use serde_json::json;

/// A category hidden from the pickers, below a storage root of its own: its id and its folder.
async fn hidden_category(
    router: &axum::Router,
    directory: &std::path::Path,
) -> (String, std::path::PathBuf) {
    let root = directory.join("library");
    std::fs::create_dir_all(&root).expect("library directory");
    let root = dunce::canonicalize(&root).expect("canonical library directory");
    let (status, created) = post_json(
        router,
        "/api/v1/storage-roots",
        json!({ "name": "library", "path": root.to_string_lossy(), "is_default": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, category) = post_json(
        router,
        "/api/v1/categories",
        json!({
            "name": "NSW - Updates",
            "color": "#336699",
            "storage_root_id": created["id"],
            "relative_path": "nsw_updates",
            "is_default": false,
            "selectable": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{category}");
    let id = category["id"].as_str().expect("category id").to_owned();
    (id, root.join("nsw_updates"))
}

/// An automation on `trigger` that puts the package into `category_id`.
async fn set_category_on(router: &axum::Router, trigger: &str, category_id: &str) -> String {
    let (status, created) = post_json(
        router,
        "/api/v1/automations",
        json!({
            "name": "NSW - Update",
            "enabled": true,
            "trigger": trigger,
            "condition": { "type": "always" },
            "actions": [{ "kind": "set_category", "category_id": category_id }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("id").to_owned()
}

/// A package in `incoming/<name>` with one file row, whose payload is already on disk.
async fn package_with(
    harness: &common::Harness,
    directory: &std::path::Path,
    file_name: &str,
) -> (PackageId, rd_core::DownloadId, std::path::PathBuf) {
    let name = "1000xRESIST_Update_v1.0.9_NSW";
    let old = directory.join("incoming").join(name);
    std::fs::create_dir_all(&old).expect("package folder");
    std::fs::write(old.join(file_name), b"payload").expect("payload");
    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: old.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let file = harness
        .database
        .create_download(common_download(package.id, file_name))
        .await
        .expect("download");
    (package.id, file.id, old)
}

async fn transition(harness: &common::Harness, id: rd_core::DownloadId, states: &[DownloadState]) {
    for state in states {
        harness
            .database
            .transition_download(id, *state)
            .await
            .expect("transition");
    }
}

/// Waits until everything named is in `new` and the old package folder is gone.
async fn await_moved(new: &std::path::Path, old: &std::path::Path, files: &[&str]) {
    eventually(
        WAIT,
        "the package never reached its new category",
        || async move {
            (files.iter().all(|file| new.join(file).exists()) && !old.exists()).then_some(())
        },
    )
    .await;
}

#[tokio::test]
async fn set_category_on_a_finished_package_moves_it_into_a_hidden_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (category_id, category_path) = hidden_category(&harness.router, directory.path()).await;
    let automation = set_category_on(&harness.router, "package_completed", &category_id).await;
    let (package, _, old) = package_with(&harness, directory.path(), "update.nsp").await;
    // Unpacked output: no download row, found by walking the folder.
    std::fs::create_dir_all(old.join("unpacked")).expect("output folder");
    std::fs::write(old.join("unpacked").join("game.nsp"), b"output").expect("output");

    harness
        .database
        .set_package_state(package, PackageState::Completed, None, None, None)
        .await
        .expect("completed");

    wait_for_runs_of(&harness.router, &automation, 1, "set_category never ran").await;
    let new = category_path.join("1000xRESIST_Update_v1.0.9_NSW");
    await_moved(&new, &old, &["update.nsp", "unpacked/game.nsp"]).await;
}

#[tokio::test]
async fn set_category_during_post_processing_moves_once_and_after_the_pipeline() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let witness = super::automation_for(&harness.router, "intake_received").await;
    let (category_id, category_path) = hidden_category(&harness.router, directory.path()).await;
    let automation = set_category_on(&harness.router, "download_completed", &category_id).await;
    let (package, file, old) = package_with(&harness, directory.path(), "update.part1.rar").await;
    // The transfer, up to its last step.
    transition(
        &harness,
        file,
        &[
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
        ],
    )
    .await;
    // The pipeline already owns the folder when the file's completion is read, as on the
    // owner's instance (the package was `postprocessing` when the first run started). Written
    // only now: each transfer step derives the package's state from its files, and `resolving`
    // turned a `postprocessing` package back into `downloading` -- the completion listener then
    // ran a real pipeline on the stand-in archive, which failed and released the move.
    harness
        .database
        .set_package_state(
            package,
            PackageState::Postprocessing,
            Some(rd_core::PostprocessStage::Extracting),
            None,
            None,
        )
        .await
        .expect("postprocessing");
    transition(&harness, file, &[DownloadState::Completed]).await;

    let router = &harness.router;
    let runs_of = format!("/api/v1/automations/runs?automation_id={automation}");
    let runs_of = runs_of.as_str();
    eventually(WAIT, "set_category never finished", || async move {
        let (_, runs) = get_json(router, runs_of).await;
        runs.as_array()
            .is_some_and(|runs| runs.iter().any(|run| run["state"] == "completed"))
            .then_some(())
    })
    .await;
    let new = category_path.join("1000xRESIST_Update_v1.0.9_NSW");
    let row = harness
        .database
        .get_package(package)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(
        row.destination,
        new.to_string_lossy(),
        "the row names the new folder"
    );
    assert!(
        old.join("update.part1.rar").exists(),
        "the archive left mid-pipeline"
    );
    assert!(!new.exists(), "data moved into the new folder mid-pipeline");

    // The unpack: its output, the volume's way back, then the end of the pipeline.
    std::fs::write(old.join("update.nsp"), b"output").expect("output");
    transition(
        &harness,
        file,
        &[DownloadState::Extracting, DownloadState::Completed],
    )
    .await;
    harness
        .database
        .set_package_state(package, PackageState::Completed, None, None, None)
        .await
        .expect("completed");

    await_moved(&new, &old, &["update.part1.rar", "update.nsp"]).await;
    harness.database.broadcast(intake("after"));
    wait_for_runs_of(
        &harness.router,
        &witness,
        1,
        "the engine never read past the events",
    )
    .await;
    assert_eq!(
        run_count(&harness.router, &automation).await,
        1,
        "the return from unpacking is no second download_completed"
    );
}

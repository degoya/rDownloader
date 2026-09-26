//! Two things the download list promises: a package keeps a folder of its own when its
//! category changes, and a job can be started over from zero.

mod common;

use axum::http::StatusCode;

/// Creates the storage root the categories live under.
async fn storage_root(
    router: &axum::Router,
    directory: &std::path::Path,
) -> (String, std::path::PathBuf) {
    let root = directory.join("library");
    std::fs::create_dir_all(&root).expect("library directory");
    // The service stores the root canonicalised, and so do the expectations built from it: a
    // Windows temp dir can arrive as an 8.3 short name (`RUNNER~1`) the service spells out.
    let root = dunce::canonicalize(&root).expect("canonical library directory");
    let (status, created) = common::post_json(
        router,
        "/api/v1/storage-roots",
        serde_json::json!({ "name": "library", "path": root.to_string_lossy(), "is_default": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (created["id"].as_str().expect("root id").to_owned(), root)
}

/// Creates one category below `root_id`, returning its id and the directory it resolves to.
async fn category(
    router: &axum::Router,
    root_id: &str,
    root: &std::path::Path,
    name: &str,
) -> (String, std::path::PathBuf) {
    let (status, created) = common::post_json(
        router,
        "/api/v1/categories",
        serde_json::json!({
            "name": name,
            "color": "#336699",
            "storage_root_id": root_id,
            "relative_path": name,
            "is_default": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (
        created["id"].as_str().expect("category id").to_owned(),
        root.join(name),
    )
}

/// Queues one direct download and returns `(download id, package id)`.
async fn queued_download(router: &axum::Router) -> (String, String) {
    let (status, created) = common::post_json(
        router,
        "/api/v1/downloads",
        serde_json::json!({
            "url": "https://example.invalid/movie.mkv",
            "package_name": "Example Package"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (_, packages) = common::get_json(router, "/api/v1/packages").await;
    (
        created["id"].as_str().expect("download id").to_owned(),
        packages[0]["id"].as_str().expect("package id").to_owned(),
    )
}

#[tokio::test]
async fn a_category_change_gives_the_package_a_folder_of_its_own() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (root_id, root) = storage_root(&harness.router, directory.path()).await;
    let (category_id, category_path) = category(&harness.router, &root_id, &root, "movies").await;
    let (_, package_id) = queued_download(&harness.router).await;

    let (status, updated) = common::post_json(
        &harness.router,
        "/api/v1/packages/bulk",
        serde_json::json!({ "ids": [package_id], "category_id": category_id }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{updated}");
    let destination = updated[0]["destination"].as_str().expect("destination");
    assert_eq!(
        destination,
        category_path.join("Example Package").to_string_lossy(),
        "the package lands in <category>/<package>, not in the category root"
    );
}

#[tokio::test]
async fn a_finished_package_can_still_be_moved_to_another_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (root_id, root) = storage_root(&harness.router, directory.path()).await;
    let (first_id, first_path) = category(&harness.router, &root_id, &root, "movies").await;
    let (second_id, second_path) = category(&harness.router, &root_id, &root, "series").await;
    let (download_id, package_id) = queued_download(&harness.router).await;

    // Park it in the first category and put a finished payload where the download would be.
    let (status, _) = common::post_json(
        &harness.router,
        "/api/v1/packages/bulk",
        serde_json::json!({ "ids": [package_id], "category_id": first_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let package_directory = first_path.join("Example Package");
    std::fs::create_dir_all(&package_directory).expect("package directory");
    std::fs::write(package_directory.join("movie.mkv"), b"payload").expect("payload");
    let id: rd_core::DownloadId = download_id.parse().expect("download id");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        harness
            .database
            .transition_download(id, state)
            .await
            .expect("transition");
    }

    // Finished packages used to be refused with `package.completed_category_locked`.
    let (status, updated) = common::post_json(
        &harness.router,
        "/api/v1/packages/bulk",
        serde_json::json!({ "ids": [package_id], "category_id": second_id }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{updated}");
    let moved = second_path.join("Example Package");
    assert_eq!(
        updated[0]["destination"].as_str().expect("destination"),
        moved.to_string_lossy()
    );
    assert_eq!(
        std::fs::read(moved.join("movie.mkv")).expect("moved payload"),
        b"payload",
        "the data follows the package into its new category"
    );
    assert!(
        !package_directory.exists(),
        "and the emptied folder does not stay behind in the old category"
    );
}

/// A finished package is more than the file its download row names: unpacking leaves output
/// that has no row at all, and the archive parts frequently sit in a subfolder. Both used to
/// stay behind while the package record claimed to have moved, so the new category held the
/// payload and the old one kept everything else.
#[tokio::test]
async fn moving_a_category_carries_the_whole_package_folder_over() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (root_id, root) = storage_root(&harness.router, directory.path()).await;
    let (first_id, first_path) = category(&harness.router, &root_id, &root, "movies").await;
    let (second_id, second_path) = category(&harness.router, &root_id, &root, "series").await;
    let (download_id, package_id) = queued_download(&harness.router).await;

    let (status, _) = common::post_json(
        &harness.router,
        "/api/v1/packages/bulk",
        serde_json::json!({ "ids": [package_id], "category_id": first_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let package_directory = first_path.join("Example Package");
    std::fs::create_dir_all(package_directory.join("Sample")).expect("subfolder");
    // The file the download row names — the only one the old code moved.
    std::fs::write(package_directory.join("movie.mkv"), b"payload").expect("payload");
    // Extraction output: found by walking the directory, never given a download row. Not an
    // `.nfo` or `.sfv` — the cleanup step removes those on purpose, so they would prove nothing.
    std::fs::write(package_directory.join("movie.srt"), b"subs").expect("subtitles");
    // An archive part left in a subfolder, which was never descended into.
    std::fs::write(package_directory.join("Sample").join("movie.r01"), b"part").expect("part");

    let id: rd_core::DownloadId = download_id.parse().expect("download id");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        harness
            .database
            .transition_download(id, state)
            .await
            .expect("transition");
    }

    let (status, updated) = common::post_json(
        &harness.router,
        "/api/v1/packages/bulk",
        serde_json::json!({ "ids": [package_id], "category_id": second_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");

    let moved = second_path.join("Example Package");
    assert_eq!(
        std::fs::read(moved.join("movie.mkv")).expect("payload"),
        b"payload"
    );
    assert_eq!(
        std::fs::read(moved.join("movie.srt")).expect("extracted output follows"),
        b"subs",
        "output without a download row must move too"
    );
    assert_eq!(
        std::fs::read(moved.join("Sample").join("movie.r01")).expect("subfolder follows"),
        b"part",
        "a subfolder and its archive parts must move too"
    );
    assert!(
        !package_directory.exists(),
        "nothing may be left behind in the old category"
    );
}

#[tokio::test]
async fn a_finished_download_can_be_reset_and_keeps_its_file_unless_asked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (download_id, _) = queued_download(&harness.router).await;
    let id: rd_core::DownloadId = download_id.parse().expect("download id");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        harness
            .database
            .transition_download(id, state)
            .await
            .expect("transition");
    }
    let (_, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    let destination =
        std::path::PathBuf::from(packages[0]["destination"].as_str().expect("destination"));
    std::fs::create_dir_all(&destination).expect("destination");
    let payload = destination.join("movie.mkv");
    std::fs::write(&payload, b"payload").expect("payload");

    let (status, message) = common::post_json(
        &harness.router,
        &format!("/api/v1/downloads/{download_id}/reset"),
        serde_json::json!({ "delete_completed_files": false }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{message}");
    assert_eq!(message["code"], "download.reset");
    let current = harness
        .database
        .get_download(id)
        .await
        .expect("download")
        .expect("still queued");
    assert_eq!(
        current.state,
        rd_core::DownloadState::Queued,
        "a finished job goes back to the queue even though Completed -> Queued is no legal \
         lifecycle move"
    );
    assert_eq!(current.committed_bytes.get(), 0);
    assert_eq!(current.retry_count, 0);
    assert!(payload.exists(), "the finished file is kept by default");

    let (status, message) = common::post_json(
        &harness.router,
        &format!("/api/v1/downloads/{download_id}/reset"),
        serde_json::json!({ "delete_completed_files": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{message}");
    assert!(
        !payload.exists(),
        "and removed when the caller asks for it explicitly"
    );
}

#[tokio::test]
async fn the_bulk_endpoint_resets_several_files_at_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (download_id, _) = queued_download(&harness.router).await;
    // A running job is refused a reset, and rightly so; the parked harness never dispatches the
    // row (RD-108-15), and pausing is the one non-running state a caller can ask for.
    let (status, paused) = common::post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        serde_json::json!({ "ids": [download_id], "action": "pause" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    await_download_state(&harness.router, &download_id, "paused").await;

    let (status, result) = common::post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        serde_json::json!({ "ids": [download_id], "action": "reset" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["affected"], 1);
    assert_eq!(
        result["errors"].as_array().map(Vec::len),
        Some(0),
        "{result}"
    );
}

/// Waits until `id` reads `state`, and says what it does read when it never gets there.
async fn await_download_state(router: &axum::Router, id: &str, state: &str) {
    for _ in 0..200 {
        let (_, downloads) = common::get_json(router, "/api/v1/downloads").await;
        let current = downloads
            .as_array()
            .and_then(|list| list.iter().find(|row| row["id"] == id))
            .map(|row| row["state"].clone());
        if current.as_ref().and_then(serde_json::Value::as_str) == Some(state) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (_, downloads) = common::get_json(router, "/api/v1/downloads").await;
    panic!("the download never reached {state}: {downloads}");
}

/// Parks the package in `category_id`, puts a finished payload in its folder, and completes it.
async fn finished_package_in(
    harness: &common::Harness,
    category_id: &str,
    category_path: &std::path::Path,
) -> (String, std::path::PathBuf) {
    let (download_id, package_id) = queued_download(&harness.router).await;
    let (status, _) = common::post_json(
        &harness.router,
        "/api/v1/packages/bulk",
        serde_json::json!({ "ids": [package_id], "category_id": category_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let package_directory = category_path.join("Example Package");
    std::fs::create_dir_all(&package_directory).expect("package directory");
    std::fs::write(package_directory.join("movie.mkv"), b"payload").expect("payload");
    let id: rd_core::DownloadId = download_id.parse().expect("download id");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        harness
            .database
            .transition_download(id, state)
            .await
            .expect("transition");
    }
    // Completing the file starts post-processing, and while it runs the package refuses a
    // rename or move as busy; on a Windows runner the test's next request landed inside that
    // window (2026-09-26). Wait for the package to settle rather than for a deadline.
    await_package_settled(&harness.router, &package_id).await;
    (package_id, package_directory)
}

/// Waits until package `id` is out of post-processing, and says what it reads when it never is.
async fn await_package_settled(router: &axum::Router, id: &str) {
    let mut last = serde_json::Value::Null;
    for _ in 0..200 {
        let (_, packages) = common::get_json(router, "/api/v1/packages").await;
        last = packages
            .as_array()
            .and_then(|list| list.iter().find(|row| row["id"] == id))
            .map(|row| row["state"].clone())
            .unwrap_or_default();
        if last.as_str() != Some("postprocessing") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the package never left post-processing, last read {last}");
}

/// The gap this endpoint closes (RD-106-13): `PATCH /packages/{id}` renames the label and
/// leaves the folder called whatever it was called when the download started.
#[tokio::test]
async fn renaming_a_package_folder_moves_the_data_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (root_id, root) = storage_root(&harness.router, directory.path()).await;
    let (category_id, category_path) = category(&harness.router, &root_id, &root, "movies").await;
    let (package_id, package_directory) =
        finished_package_in(&harness, &category_id, &category_path).await;

    let (status, updated) = common::post_json(
        &harness.router,
        &format!("/api/v1/packages/{package_id}/folder"),
        serde_json::json!({ "name": "Renamed Package" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{updated}");
    let renamed = category_path.join("Renamed Package");
    assert_eq!(updated["name"].as_str(), Some("Renamed Package"));
    assert_eq!(
        updated["destination"].as_str().expect("destination"),
        renamed.to_string_lossy(),
        "the folder stays beside the one it replaces, inside the same category"
    );
    assert_eq!(
        std::fs::read(renamed.join("movie.mkv")).expect("moved payload"),
        b"payload",
        "the data did not follow the folder"
    );
    assert!(
        !package_directory.exists(),
        "the folder under the old name stayed behind"
    );
}

/// The one place this feature deliberately departs from the rest of the codebase.
///
/// Everywhere else a name that is taken is stepped around with `collision_free_path`, which
/// appends ` (1)`. That is right when a file lands somewhere unattended. It is wrong here:
/// somebody typed this name, and quietly storing a different one hands them a folder they did
/// not ask for and cannot tell apart from the one they meant.
#[tokio::test]
async fn a_folder_name_that_is_already_taken_is_refused_rather_than_worked_around() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (root_id, root) = storage_root(&harness.router, directory.path()).await;
    let (category_id, category_path) = category(&harness.router, &root_id, &root, "movies").await;
    let (package_id, package_directory) =
        finished_package_in(&harness, &category_id, &category_path).await;
    let occupied = category_path.join("Renamed Package");
    std::fs::create_dir_all(&occupied).expect("occupied folder");
    std::fs::write(occupied.join("somebody-elses.mkv"), b"theirs").expect("their payload");

    let (status, error) = common::post_json(
        &harness.router,
        &format!("/api/v1/packages/{package_id}/folder"),
        serde_json::json!({ "name": "Renamed Package" }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"].as_str(), Some("package.folder_exists"));
    assert_eq!(
        std::fs::read(occupied.join("somebody-elses.mkv")).expect("their payload"),
        b"theirs",
        "the other folder was written into"
    );
    assert!(
        package_directory.join("movie.mkv").exists(),
        "the refused rename moved data anyway"
    );
    let (_, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    assert_eq!(
        packages[0]["destination"].as_str().expect("destination"),
        package_directory.to_string_lossy(),
        "the refused rename still wrote the row"
    );
}

/// Renaming while a file is transferring is out of scope by decision: the runner holds an open
/// handle in the old folder, and moving it out from under the runner costs the resume point.
#[tokio::test]
async fn a_package_that_is_still_transferring_is_not_renamed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (root_id, root) = storage_root(&harness.router, directory.path()).await;
    let (category_id, category_path) = category(&harness.router, &root_id, &root, "movies").await;
    let (download_id, package_id) = queued_download(&harness.router).await;
    let (status, _) = common::post_json(
        &harness.router,
        "/api/v1/packages/bulk",
        serde_json::json!({ "ids": [package_id], "category_id": category_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id: rd_core::DownloadId = download_id.parse().expect("download id");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
    ] {
        harness
            .database
            .transition_download(id, state)
            .await
            .expect("transition");
    }

    let (status, error) = common::post_json(
        &harness.router,
        &format!("/api/v1/packages/{package_id}/folder"),
        serde_json::json!({ "name": "Renamed Package" }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"].as_str(), Some("package.folder_busy"));
    assert!(
        !category_path.join("Renamed Package").exists(),
        "a running package was renamed anyway"
    );
}

/// The name goes through `rd_files::sanitize_file_name`, the same rules the host file rename
/// uses — so a separator in it becomes part of the folder name rather than a step out of the
/// category directory.
#[tokio::test]
async fn a_folder_name_cannot_walk_the_package_out_of_its_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (root_id, root) = storage_root(&harness.router, directory.path()).await;
    let (category_id, category_path) = category(&harness.router, &root_id, &root, "movies").await;
    let (package_id, _) = finished_package_in(&harness, &category_id, &category_path).await;

    let (status, updated) = common::post_json(
        &harness.router,
        &format!("/api/v1/packages/{package_id}/folder"),
        serde_json::json!({ "name": "../../escaped" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{updated}");
    let destination = updated["destination"].as_str().expect("destination");
    assert_eq!(
        std::path::Path::new(destination).parent(),
        Some(category_path.as_path()),
        "the rename left the category directory: {destination}"
    );
}

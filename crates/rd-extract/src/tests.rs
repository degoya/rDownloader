use std::{io::Write, time::Duration};

use rd_core::{DownloadId, DownloadState, PackageId, PostprocessKind};
use rd_db::{Database, NewDownload, NewPackage};
use zip::write::SimpleFileOptions;

use crate::{ExtractionConfig, ExtractionService, ExtractionTrigger};

pub(crate) fn write_zip(path: &std::path::Path, options: SimpleFileOptions, name: &str) {
    let file = std::fs::File::create(path).expect("create ZIP");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(name, options).expect("start member");
    zip.write_all(b"payload").expect("write member");
    zip.finish().expect("finish ZIP");
}

pub(crate) fn zip_bytes(member_name: &str, content: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(member_name, SimpleFileOptions::default())
        .expect("start member");
    zip.write_all(content).expect("write member");
    zip.finish().expect("finish ZIP").into_inner()
}

/// `level1.zip ⊃ level2.zip ⊃ … ⊃ payload.txt`: extracting `levels` times reaches the payload.
pub(crate) fn nested_zip(levels: usize) -> Vec<u8> {
    let mut name = "payload.txt".to_owned();
    let mut bytes = b"payload".to_vec();
    for level in (1..=levels).rev() {
        bytes = zip_bytes(&name, &bytes);
        name = format!("level{level}.zip");
    }
    bytes
}

/// Creates a completed one-file package holding `level1.zip` with the given nesting depth.
pub(crate) async fn seed_nested_package(
    database: &Database,
    destination: &std::path::Path,
    levels: usize,
) -> PackageId {
    std::fs::create_dir_all(destination).expect("destination");
    std::fs::write(destination.join("level1.zip"), nested_zip(levels)).expect("archive");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "nested".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let file = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/level1.zip".parse().expect("URL"),
            file_name: "level1.zip".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
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
    for state in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
        DownloadState::Completed,
    ] {
        database
            .transition_download(file.id, state)
            .await
            .expect("transition");
    }
    package.id
}

/// Waits until `service` has finished its job for `package_id`, the last step included.
///
/// The package state and the step rows are written while the job still runs: a test that stops
/// at them and shuts the service down (which only cancels the queue, never a running job) left
/// the job writing on, and a second run started right after met the first one's late writes
/// (RD-170-16, seen on Windows only).
pub(crate) async fn wait_until_finished(service: &ExtractionService, package_id: PackageId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while service.pending().await.contains(&package_id) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the post-processing job finished");
}

pub(crate) async fn run_extraction(
    database: &Database,
    temp: &std::path::Path,
    package_id: PackageId,
) {
    let state = run_extraction_to_end(database, temp, package_id).await;
    assert_eq!(state, rd_core::PackageState::Completed);
}

/// Runs the pipeline and returns whichever terminal state the package reached.
pub(crate) async fn run_extraction_to_end(
    database: &Database,
    temp: &std::path::Path,
    package_id: PackageId,
) -> rd_core::PackageState {
    run_extraction_with(database, temp, package_id, ExtractionTrigger::Manual).await
}

/// The same, with the trigger spelled out — `Force` is the "post-process anyway" action.
pub(crate) async fn run_extraction_with(
    database: &Database,
    temp: &std::path::Path,
    package_id: PackageId,
    trigger: ExtractionTrigger,
) -> rd_core::PackageState {
    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    service.request(package_id, trigger).await.expect("request");
    let state = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let package = database
                .list_packages()
                .await
                .expect("packages")
                .into_iter()
                .find(|package| package.id == package_id)
                .expect("package");
            if matches!(
                package.state,
                rd_core::PackageState::Completed | rd_core::PackageState::Failed
            ) {
                break package.state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("extraction finished");
    wait_until_finished(&service, package_id).await;
    service.shutdown().await;
    state
}

#[tokio::test]
async fn recursive_unpack_extracts_nested_archives_and_deletes_intermediates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "recursive_unpack": true }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    let package_id = seed_nested_package(&database, &destination, 2).await;
    run_extraction(&database, temp.path(), package_id).await;
    assert_eq!(
        std::fs::read(destination.join("payload.txt")).ok(),
        Some(b"payload".to_vec())
    );
    // The inner archive is an intermediate and gets deleted; the original volume follows
    // the package's level (Unpack, no delete) and stays.
    assert!(!destination.join("level2.zip").exists());
    assert!(destination.join("level1.zip").exists());
}

#[tokio::test]
async fn recursive_unpack_stops_at_the_depth_cap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "recursive_unpack": true }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    // Five levels, but only the initial pass plus 3 recursive passes run.
    let package_id = seed_nested_package(&database, &destination, 5).await;
    run_extraction(&database, temp.path(), package_id).await;
    assert!(destination.join("level5.zip").exists());
    assert!(!destination.join("level4.zip").exists());
    assert!(!destination.join("payload.txt").exists());
}

#[tokio::test]
async fn nested_archives_stay_untouched_without_recursive_unpack() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    let package_id = seed_nested_package(&database, &destination, 2).await;
    run_extraction(&database, temp.path(), package_id).await;
    assert!(destination.join("level2.zip").exists());
    assert!(!destination.join("payload.txt").exists());
}

/// Registers `file_name` in `destination` as a completed download of `package_id`.
pub(crate) async fn seed_completed_file(
    database: &Database,
    package_id: PackageId,
    file_name: &str,
) {
    let file = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: format!("https://example.test/{file_name}")
                .parse()
                .expect("URL"),
            file_name: file_name.to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
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
    for state in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
        DownloadState::Completed,
    ] {
        database
            .transition_download(file.id, state)
            .await
            .expect("transition");
    }
}

/// The reason a folder rename has to rewrite `postprocess_steps.source_path` (RD-106-13).
///
/// `source_path` is not a note about a step, it is the step's identity: the table is keyed by
/// `(owner_id, kind, source_path)` and `steps::find_step` looks a step up by exactly that. Left
/// pointing at the folder the package no longer has, a second run — and `extract/force` exists
/// to start one on a finished package — would not recognise its own earlier work and would
/// write a second set of rows beside the first, so "has this package been repaired yet?" would
/// start answering wrongly.
#[tokio::test]
async fn a_renamed_package_folder_does_not_give_a_second_run_a_second_set_of_steps() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("library").join("Old Name");
    let package_id = seed_nested_package(&database, &destination, 1).await;
    run_extraction(&database, temp.path(), package_id).await;

    let before = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    assert!(
        !before.is_empty(),
        "the first run recorded nothing to rename"
    );

    // Rename the way the endpoint does: the row first, the disk after.
    let renamed = temp.path().join("library").join("New Name");
    database
        .rename_package_directory(
            package_id,
            "New Name".to_owned(),
            renamed.to_string_lossy().into_owned(),
        )
        .await
        .expect("rename")
        .expect("package");
    std::fs::rename(&destination, &renamed).expect("move the folder");

    let after_rename = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    assert_eq!(
        after_rename.len(),
        before.len(),
        "the rename itself must not add or drop a step"
    );
    let old_prefix = destination.to_string_lossy().into_owned();
    for step in &after_rename {
        assert!(
            !step.source_path.starts_with(&old_prefix),
            "a step still points into the old folder: {}",
            step.source_path
        );
        assert!(
            step.output_path
                .as_deref()
                .is_none_or(|path| !path.starts_with(&old_prefix)),
            "a step's output still points into the old folder: {:?}",
            step.output_path
        );
    }

    // `run_extraction_with` would return at once here: the package is already `Completed`, so
    // its wait loop is satisfied before the pipeline has done anything. The forced run is
    // therefore driven by hand and waited on by the mark the last step leaves behind.
    let mark = chrono::Utc::now();
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
    service
        .request(package_id, ExtractionTrigger::Force)
        .await
        .expect("force");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // Cleanup is the last step of the pipeline, so its fresh timestamp means every
            // unpack row the run was going to write has been written.
            let done = database
                .list_postprocess_steps(&package_id.to_string())
                .await
                .expect("steps")
                .iter()
                .any(|step| step.kind == PostprocessKind::Cleanup && step.updated_at > mark);
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the forced run finished");
    service.shutdown().await;

    let after_force = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let duplicated: Vec<&str> = after_force
        .iter()
        .filter(|step| step.source_path.starts_with(&old_prefix))
        .map(|step| step.source_path.as_str())
        .collect();
    assert!(
        duplicated.is_empty(),
        "the forced run resurrected rows under the old folder: {duplicated:?}"
    );
    assert_eq!(
        after_force.len(),
        after_rename.len(),
        "the forced run wrote a second set of steps instead of finding its own: {:?}",
        after_force
            .iter()
            .map(|step| (step.kind, step.source_path.as_str()))
            .collect::<Vec<_>>()
    );
}

/// The pipeline internals, without the job loop: these tests assert what one pass leaves
/// behind, and the interesting pass is the one that does *not* end in a terminal state.
pub(crate) fn extraction_inner(
    database: &Database,
    temp: &std::path::Path,
) -> std::sync::Arc<crate::Inner> {
    // The receiver is dropped straight away: these tests call the pipeline directly and
    // nothing ever queues another job through this handle.
    let (jobs, _) = tokio::sync::mpsc::channel(8);
    std::sync::Arc::new(crate::Inner {
        database: database.clone(),
        config: ExtractionConfig {
            default_passwords_file: temp.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: temp.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
        hold: rd_core::PostprocessHold::new(),
        jobs,
        in_flight: tokio::sync::Mutex::new(crate::InFlight::default()),
        shutdown: tokio_util::sync::CancellationToken::new(),
        plugin_steps: None,
        storage: None,
        objects: None,
        direct: crate::direct_unpack::DirectUnpacks::default(),
    })
}

/// Engine audit 1.8, finding 7: a manual trigger while a job waits queues a second one; the
/// first ending must leave the package pending until the second has run too.
#[test]
fn a_package_stays_pending_until_its_last_job_has_run() {
    use crate::{ExtractionTrigger, InFlight};

    let mut in_flight = InFlight::default();
    let package = PackageId::new();
    assert!(in_flight.claim(package, ExtractionTrigger::Auto));
    assert!(
        !in_flight.claim(package, ExtractionTrigger::Auto),
        "an automatic trigger leaves a queued job alone"
    );
    assert!(in_flight.claim(package, ExtractionTrigger::Manual));
    in_flight.release(package);
    assert!(
        in_flight.packages().contains(&package),
        "one job is still to come"
    );
    assert!(!in_flight.claim(package, ExtractionTrigger::Auto));
    in_flight.release(package);
    assert!(in_flight.packages().is_empty());
    in_flight.release(package);
    assert!(
        in_flight.packages().is_empty(),
        "a stray release changes nothing"
    );
}

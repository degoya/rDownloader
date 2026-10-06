use std::path::{Path, PathBuf};

use anyhow::Context;
use rd_core::{DownloadFile, DownloadState};

use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

/// A scheduler over a temporary database, plus one paused package below `storage/`.
pub(super) async fn paused_package(directory: &Path) -> (SchedulerHandle, DownloadFile, PathBuf) {
    paused_package_of(directory, rd_core::DownloadKind::Http).await
}

/// [`paused_package`] with one file of `kind`.
async fn paused_package_of(
    directory: &Path,
    kind: rd_core::DownloadKind,
) -> (SchedulerHandle, DownloadFile, PathBuf) {
    let database = rd_db::Database::open(directory.join("scheduler-test.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database,
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    // The start carries on unfinished moves in the background; under load it ran after a
    // case had set up a move of its own and finished it first (T13, as `b8a3643b` for the
    // crash cases). Let it run before the case begins.
    scheduler.storage_recovery_finished().await;
    let (_package, files) = scheduler
        .enqueue_package(
            PackageSpec {
                name: "Example Package".to_owned(),
                destination: directory.join("storage"),
                category_id: None,
                priority: rd_core::DownloadPriority::default(),
                password: None,
                // Paused, so the supervisor leaves the file alone while the test works on it.
                start_paused: true,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            },
            vec![FileSpec {
                source: "https://example.invalid/file.bin".parse().expect("url"),
                file_name: "file.bin".to_owned(),
                size: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                kind,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                skipped: false,
                enrichment: Vec::new(),
                secret_fragment: None,
                source_set: None,
            }],
        )
        .await
        .expect("enqueue");
    let file = files.into_iter().next().expect("one file");
    let destination = directory.join("storage").join("Example Package");
    (scheduler, file, destination)
}

/// Points the package at `destination` the way a category change does, recording where its
/// data used to be.
async fn set_destination(
    scheduler: &SchedulerHandle,
    package_id: rd_core::PackageId,
    destination: &Path,
) {
    scheduler
        .database
        .update_packages(
            vec![package_id],
            rd_db::PackageChange {
                category: Some(rd_db::CategoryAssignment {
                    category_id: None,
                    destinations: std::collections::HashMap::from([(
                        package_id,
                        destination.to_string_lossy().into_owned(),
                    )]),
                }),
                ..Default::default()
            },
        )
        .await
        .expect("category change");
}

#[tokio::test]
async fn a_category_change_carries_the_package_folder_over() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    tokio::fs::write(destination.join(&file.file_name), b"payload")
        .await
        .expect("payload");

    let moved = temporary.path().join("movies").join("Example Package");
    set_destination(&scheduler, file.package_id, &moved).await;
    scheduler
        .relocate_package(file.package_id)
        .await
        .expect("relocate");

    assert_eq!(
        tokio::fs::read(moved.join(&file.file_name))
            .await
            .expect("moved payload"),
        b"payload",
        "the file follows its package into the new category folder"
    );
    assert!(
        !destination.exists(),
        "the emptied package folder does not stay behind"
    );
    assert_eq!(
        scheduler
            .database
            .package_previous_destination(file.package_id)
            .await
            .expect("previous destination"),
        None,
        "and the outstanding move is marked as done"
    );
}

#[tokio::test]
async fn a_job_holding_both_a_payload_and_a_checkpoint_carries_both_over() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    let staging = destination.join(".rdownloader");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::write(destination.join(&file.file_name), b"payload")
        .await
        .expect("payload");
    let part = staging.join(format!("{}.part", file.id));
    tokio::fs::write(&part, b"partial")
        .await
        .expect("part file");

    let moved = temporary.path().join("movies").join("Example Package");
    set_destination(&scheduler, file.package_id, &moved).await;
    scheduler
        .relocate_package(file.package_id)
        .await
        .expect("relocate");

    assert!(
        moved.join(&file.file_name).exists(),
        "the payload moves with the package"
    );
    assert!(
        moved
            .join(".rdownloader")
            .join(format!("{}.part", file.id))
            .exists(),
        "and so does the checkpoint, which is not an alternative to the payload"
    );
    assert!(
        !destination.exists(),
        "so nothing is left to keep the old directory alive"
    );
}

/// A folder that belongs to one package moves with it, contents and all.
///
/// Not only the rows the database knows about: extracted output has no download row, and
/// archive parts can sit in a subfolder that was never descended into, so both used to
/// stay behind while the package record claimed to have moved. Anything else in a folder
/// that belongs to this package alone is treated the same way, because there is no way to
/// tell it apart from the extraction output it sits next to.
#[tokio::test]
async fn everything_in_a_package_folder_moves_with_the_package() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    tokio::fs::write(destination.join(&file.file_name), b"payload")
        .await
        .expect("payload");
    // Stands in for extracted output: real content, no row in the database.
    let unknown = destination.join("notes.txt");
    tokio::fs::write(&unknown, b"mine")
        .await
        .expect("extra file");

    let moved = temporary.path().join("movies").join("Example Package");
    set_destination(&scheduler, file.package_id, &moved).await;
    scheduler
        .relocate_package(file.package_id)
        .await
        .expect("relocate");

    assert!(
        moved.join(&file.file_name).exists(),
        "the payload moves with the package"
    );
    assert!(
        moved.join("notes.txt").exists(),
        "and so does what the database never knew about"
    );
    assert!(
        !destination.exists(),
        "leaving nothing behind to keep the old directory alive"
    );
}

#[tokio::test]
async fn a_row_that_still_points_at_the_category_root_keeps_the_category_folder() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _) = paused_package(temporary.path()).await;
    // The shape of a package written before the category change built a folder per package:
    // its destination *is* the category directory, shared with everything else in it.
    let category = temporary.path().join("storage");
    set_destination(&scheduler, file.package_id, &category).await;
    scheduler
        .database
        .clear_package_previous_destination(file.package_id)
        .await
        .expect("clear");
    tokio::fs::create_dir_all(&category)
        .await
        .expect("category");
    tokio::fs::write(category.join(&file.file_name), b"payload")
        .await
        .expect("payload");
    let sibling = category.join("another-package.bin");
    tokio::fs::write(&sibling, b"other").await.expect("sibling");

    let moved = category.join("Example Package");
    set_destination(&scheduler, file.package_id, &moved).await;
    scheduler
        .relocate_package(file.package_id)
        .await
        .expect("relocate");

    assert_eq!(
        tokio::fs::read(moved.join(&file.file_name))
            .await
            .expect("moved payload"),
        b"payload",
        "only the package's own file is carried into its new folder"
    );
    assert!(
        sibling.exists(),
        "a file belonging to another package stays where it is"
    );
    assert!(
        category.is_dir(),
        "and the category directory itself is never swept away"
    );
}

#[tokio::test]
async fn resetting_discards_the_partial_file_and_queues_the_job_again() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    let staging = destination.join(".rdownloader");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    let part = staging.join(format!("{}.part", file.id));
    tokio::fs::write(&part, b"partial")
        .await
        .expect("part file");

    scheduler.reset(file.id, false).await.expect("reset");

    assert!(!part.exists(), "the checkpoint file is discarded");
    let current = scheduler
        .database
        .get_download(file.id)
        .await
        .expect("download")
        .expect("still there");
    assert_eq!(current.state, rd_core::DownloadState::Queued);
    assert_eq!(current.committed_bytes.get(), 0, "progress starts at zero");
    assert_eq!(current.retry_count, 0, "and so does the retry budget");
    assert!(
        !scheduler.active.lock().await.reasons.contains_key(&file.id),
        "the reset lets go of the dispatcher once the row is written back"
    );
}

/// TR-04: a reset holds the row against the dispatcher until it is written back. Let go
/// before the `.part` was deleted, a due retry started in between and resumed at the row's
/// checkpoint in a new empty file.
///
/// RA-TR-07/RA-TR-02: the real `reset` runs inside a second hold, the way two resets or a
/// reset and a removal of one row overlap, and so do the pause and cancel that each let go
/// of a stop reason when they are done. None of them may let the dispatcher at the row
/// while the outer hold is still at work.
#[tokio::test]
async fn a_dispatch_pass_leaves_a_row_alone_while_it_is_being_reset() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _) = paused_package(temporary.path()).await;

    scheduler
        .while_held(file.id, "refused", async {
            scheduler.reset(file.id, false).await?;
            scheduler.pause(file.id).await?;
            scheduler.cancel(file.id).await?;
            // Startable, as a queued row or a retry that fell due is while it is reset.
            scheduler.database.reset_download(file.id).await?;
            scheduler.schedule_runnable().await?;
            anyhow::ensure!(
                !scheduler.active.lock().await.tokens.contains_key(&file.id),
                "the dispatcher started a row in the middle of its reset"
            );
            anyhow::Ok(())
        })
        .await
        .expect("held");
    let active = scheduler.active.lock().await;
    assert!(
        !active.reasons.contains_key(&file.id) && !active.held.contains_key(&file.id),
        "and the hold ends with the work"
    );
}

/// RA-TR-02: a resume in the middle of a reset took the hold out and queued the row while
/// its `.part` was still being deleted. It is refused, and the row stays where it was.
#[tokio::test]
async fn a_resume_during_a_reset_does_not_queue_the_row() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _) = paused_package(temporary.path()).await;

    scheduler
        .while_held(file.id, "refused", async {
            let refused = scheduler.resume(file.id).await;
            anyhow::ensure!(
                refused.as_ref().is_err_and(
                    |error| rd_db::store_kind(error) == Some(rd_db::StoreErrorKind::WrongState)
                ),
                "a resume in the middle of a reset was not refused: {refused:?}"
            );
            let current = scheduler
                .database
                .get_download(file.id)
                .await?
                .context("still there")?;
            anyhow::ensure!(
                current.state == DownloadState::Paused,
                "the resume queued the row anyway: {:?}",
                current.state
            );
            anyhow::Ok(())
        })
        .await
        .expect("held");
    scheduler
        .resume(file.id)
        .await
        .expect("resume after the hold");
}

/// RA-DB-01: a Usenet file whose NZB was dropped is refused before the reset deletes
/// anything, with a reason of its own; it used to lose the finished file first and then be
/// told to pause.
#[tokio::test]
async fn a_refused_reset_of_a_usenet_file_without_its_nzb_keeps_the_file() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) =
        paused_package_of(temporary.path(), rd_core::DownloadKind::Usenet).await;
    assert!(file.nzb_file_id.is_none(), "no NZB behind the row");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let payload = destination.join(&file.file_name);
    tokio::fs::write(&payload, b"payload")
        .await
        .expect("payload");

    let refused = scheduler
        .reset(file.id, true)
        .await
        .expect_err("nothing left to fetch it from");
    assert!(
        refused.downcast_ref::<super::NzbDropped>().is_some(),
        "refused with its own reason: {refused:#}"
    );
    assert!(payload.exists(), "a refused reset deletes nothing");
    assert!(
        !scheduler.active.lock().await.held.contains_key(&file.id),
        "and holds nothing"
    );
}

/// `Episode 1` resetting must not take `Episode 10`'s or `Episode 1.5`'s stream fragments
/// with it; its own fragments and resume state go.
#[tokio::test]
async fn a_reset_discards_only_its_own_scratch_files() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (_scheduler, file, destination) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let named = |name: &str, state: DownloadState| DownloadFile {
        id: rd_core::DownloadId::new(),
        file_name: name.to_owned(),
        state,
        ..file.clone()
    };
    let own = named("Episode 1.mp4", DownloadState::Paused);
    let neighbours = [
        named("Episode 10.mp4", DownloadState::Downloading),
        named("Episode 1.5.mp4", DownloadState::Paused),
    ];
    let names = [
        "Episode 1.f137.mp4.part",
        "Episode 1.info.json.ytdl",
        "Episode 10.f137.mp4.part",
        "Episode 10.info.json.ytdl",
        "Episode 1.5.f137.mp4.part",
        "Episode 1.mp4",
    ];
    for name in names {
        tokio::fs::write(destination.join(name), b"x")
            .await
            .expect("scratch file");
    }

    super::discard_scratch_files(&destination.to_string_lossy(), &own, &neighbours)
        .await
        .expect("discard");

    let left = |name: &str| destination.join(name).exists();
    assert!(!left("Episode 1.f137.mp4.part"));
    assert!(!left("Episode 1.info.json.ytdl"));
    assert!(left("Episode 10.f137.mp4.part"), "a neighbour's fragment");
    assert!(
        left("Episode 10.info.json.ytdl"),
        "a neighbour's resume state"
    );
    assert!(
        left("Episode 1.5.f137.mp4.part"),
        "a longer stem's fragment"
    );
    assert!(left("Episode 1.mp4"), "downloaded data");

    // The same stem, still running: its fragments cannot be told apart from ours.
    tokio::fs::write(destination.join("Episode 1.f137.mp4.part"), b"x")
        .await
        .expect("scratch file");
    let running = [named("Episode 1.mkv", DownloadState::Downloading)];
    super::discard_scratch_files(&destination.to_string_lossy(), &own, &running)
        .await
        .expect("discard");
    assert!(
        left("Episode 1.f137.mp4.part"),
        "a running namesake's fragment"
    );
}

#[tokio::test]
async fn a_reset_keeps_the_finished_file_unless_it_is_asked_to_delete_it() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let payload = destination.join(&file.file_name);
    tokio::fs::write(&payload, b"payload")
        .await
        .expect("payload");

    scheduler.reset(file.id, false).await.expect("reset");
    assert!(
        payload.exists(),
        "a reset does not destroy the only copy by default"
    );

    // The first reset queued the job again, so the supervisor may already have started it:
    // it is paused, and the second reset waits until its worker has let go.
    scheduler.pause(file.id).await.expect("pause");
    let mut attempts = 0;
    while let Err(error) = scheduler.reset(file.id, true).await {
        attempts += 1;
        assert!(attempts < 200, "the paused job never let go: {error:#}");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !payload.exists(),
        "and removes it when that is what was asked for"
    );
}

/// "Clear the entire list" with its box ticked (RD-180-21): what the stopped job wrote goes,
/// the row stays until the removal that follows, and data that is not scratch stays too.
#[tokio::test]
async fn discarding_partial_data_keeps_the_row_and_everything_that_is_not_scratch() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    let staging = destination.join(".rdownloader");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    let part = staging.join(format!("{}.part", file.id));
    tokio::fs::write(&part, b"partial")
        .await
        .expect("part file");
    let fragment = destination.join("file.f137.bin.part");
    tokio::fs::write(&fragment, b"x").await.expect("fragment");
    let kept = destination.join("already-downloaded.bin");
    tokio::fs::write(&kept, b"payload").await.expect("payload");

    scheduler.discard_partial(file.id).await.expect("discard");

    assert!(!part.exists(), "the staging file goes");
    assert!(!fragment.exists(), "and the tool's scratch file with it");
    assert!(kept.exists(), "downloaded data is not scratch");
    assert!(
        scheduler
            .database
            .get_download(file.id)
            .await
            .expect("download")
            .is_some(),
        "the row is the removal's to take, after the data"
    );
}

#[tokio::test]
async fn removing_a_download_does_not_recreate_a_destination_that_was_moved_away() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    assert!(!destination.exists(), "nothing was downloaded yet");

    scheduler.remove(file.id).await.expect("remove");

    assert!(
        !destination.exists(),
        "removing a download must not create its package directory"
    );
}

#[tokio::test]
async fn removing_the_last_download_drops_the_empty_package_directory() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    let staging = destination.join(".rdownloader");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::write(staging.join(format!("{}.part", file.id)), b"partial")
        .await
        .expect("part file");

    scheduler.remove(file.id).await.expect("remove");

    assert!(
        !destination.exists(),
        "an empty package directory is cleaned up with its last file"
    );
}

#[tokio::test]
async fn a_package_directory_that_still_holds_data_survives() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let kept = destination.join("already-downloaded.bin");
    tokio::fs::write(&kept, b"payload").await.expect("payload");

    scheduler.remove(file.id).await.expect("remove");

    assert!(kept.exists(), "downloaded data is never removed");
}

/// Leaves the file `Verifying` with no worker behind it, the way a Usenet file waiting for
/// its set's PAR2 verdict sits in the queue (RD-108-24).
async fn waiting_in_verifying(scheduler: &SchedulerHandle, file: &DownloadFile) {
    for state in [DownloadState::Downloading, DownloadState::Verifying] {
        scheduler
            .database
            .transition_download(file.id, state)
            .await
            .expect("on its way");
    }
}

#[tokio::test]
async fn a_verifying_download_without_a_worker_can_be_removed() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    waiting_in_verifying(&scheduler, &file).await;

    scheduler.remove(file.id).await.expect("remove");

    assert!(
        scheduler
            .database
            .get_download(file.id)
            .await
            .expect("lookup")
            .is_none(),
        "nothing runs that the removal would have to wait for"
    );
}

#[tokio::test]
async fn a_verifying_download_without_a_worker_can_be_cancelled() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    waiting_in_verifying(&scheduler, &file).await;

    scheduler.cancel(file.id).await.expect("cancel");

    let cancelled = scheduler
        .database
        .get_download(file.id)
        .await
        .expect("lookup")
        .expect("row");
    assert_eq!(cancelled.state, DownloadState::Cancelled);
    scheduler
        .remove(file.id)
        .await
        .expect("a cancelled row is removable");
}

#[tokio::test]
async fn cancelling_a_finished_download_is_refused_with_a_reason() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    waiting_in_verifying(&scheduler, &file).await;
    scheduler
        .database
        .complete_download(file.id, file.file_name.clone(), None)
        .await
        .expect("complete");

    let error = scheduler
        .cancel(file.id)
        .await
        .expect_err("a finished download has nothing to cancel");

    assert_eq!(
        rd_db::store_kind(&error),
        Some(rd_db::StoreErrorKind::WrongState),
        "tagged, so the interface shows the reason instead of an internal error"
    );
}

#[tokio::test]
async fn resuming_a_finished_download_is_refused_with_a_reason() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    waiting_in_verifying(&scheduler, &file).await;
    scheduler
        .database
        .complete_download(file.id, file.file_name.clone(), None)
        .await
        .expect("complete");

    let error = scheduler
        .resume(file.id)
        .await
        .expect_err("a finished download has nothing to resume");

    assert_eq!(
        rd_db::store_kind(&error),
        Some(rd_db::StoreErrorKind::WrongState),
        "tagged, so a bulk resume names the reason instead of an internal error (RD-150-22)"
    );
}

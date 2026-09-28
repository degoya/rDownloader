//! RD-150-01 — every collision follows exactly one policy, and `ask` survives a restart.
//!
//! Against a real database and a real scheduler, like the mirror cases: the policy levels, the
//! prompt and the block reason are rows, and a restart reads exactly these.

use std::{ops::ControlFlow, path::Path, path::PathBuf};

use rd_core::{CollisionDecision, CollisionPolicy, DownloadFile, DownloadState, ExpectedChecksum};

use crate::{
    BlockReason, FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle,
    collision::{after_transfer, before_transfer},
};

async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("collisions.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    (scheduler, database)
}

/// One paused file, its package folder, and an existing `file.bin` in that folder.
async fn colliding(directory: &Path) -> (SchedulerHandle, rd_db::Database, DownloadFile, PathBuf) {
    let (scheduler, database) = scheduler_over(directory).await;
    let spec = PackageSpec {
        name: "release".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: true,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source: "https://example.invalid/file.bin".parse().expect("url"),
        file_name: "file.bin".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::default(),
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }];
    let (_package, files) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    let file = files.into_iter().next().expect("one file");
    let destination = directory.join("storage").join("release");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    tokio::fs::write(destination.join("file.bin"), b"existing")
        .await
        .expect("existing file");
    (scheduler, database, file, destination)
}

/// Takes the paused row to `Downloading`, a state the worker applies the policy in and the
/// dispatcher never picks up — `Queued` would race the supervisor of the scheduler under test.
async fn started(database: &rd_db::Database, file: &DownloadFile) -> DownloadFile {
    database
        .transition_download(file.id, DownloadState::Downloading)
        .await
        .expect("transition");
    state_of(database, file).await
}

/// And on to `Verifying`, where it is applied again before the promotion.
async fn verifying(database: &rd_db::Database, file: &DownloadFile) -> DownloadFile {
    started(database, file).await;
    database
        .transition_download(file.id, DownloadState::Verifying)
        .await
        .expect("transition");
    state_of(database, file).await
}

async fn state_of(database: &rd_db::Database, file: &DownloadFile) -> DownloadFile {
    database
        .get_download(file.id)
        .await
        .expect("read")
        .expect("row")
}

/// The matrix before a transfer: one row per policy, each with the one outcome it names, and
/// the existing file untouched by every one of them.
#[tokio::test]
async fn before_the_transfer_each_policy_does_what_it_names() {
    for policy in CollisionPolicy::ALL {
        let temporary = tempfile::tempdir().expect("tempdir");
        let (scheduler, database, file, destination) = colliding(temporary.path()).await;
        database
            .set_package_collision_policy(file.package_id, Some(policy))
            .await
            .expect("policy");
        let file = started(&database, &file).await;
        let part = destination.join(".rdownloader").join("x.part");

        // A stated size that differs proves a `compare` different before a byte is fetched.
        let flow = before_transfer(&scheduler, &file, &destination, &part, Some(3))
            .await
            .expect("decision");
        let row = state_of(&database, &file).await;
        match policy {
            CollisionPolicy::Rename | CollisionPolicy::Compare => {
                assert_eq!(
                    flow,
                    ControlFlow::Continue(destination.join("file (1).bin")),
                    "{policy:?}"
                );
                assert_eq!(row.file_name, "file (1).bin", "{policy:?}");
            }
            CollisionPolicy::Overwrite => {
                assert_eq!(flow, ControlFlow::Continue(destination.join("file.bin")));
                assert_eq!(row.file_name, "file.bin");
            }
            CollisionPolicy::Skip => {
                assert_eq!(flow, ControlFlow::Break(()));
                assert_eq!(row.state, DownloadState::Failed);
                assert_eq!(
                    row.last_error.and_then(|failure| failure.code).as_deref(),
                    Some(rd_core::CODE_COLLISION_SKIPPED)
                );
            }
            CollisionPolicy::Ask => {
                assert_eq!(flow, ControlFlow::Break(()));
                assert_eq!(row.state, DownloadState::Blocked);
                let blocked = database
                    .downloads_blocked_by(BlockReason::CollisionAsk.as_str())
                    .await
                    .expect("blocked");
                assert_eq!(blocked, vec![file.id]);
                assert!(
                    database
                        .collision_prompt(file.id)
                        .await
                        .expect("prompt")
                        .is_some()
                );
            }
        }
        assert_eq!(
            tokio::fs::read(destination.join("file.bin"))
                .await
                .expect("existing"),
            b"existing",
            "{policy:?} touched the existing file before the transfer"
        );
    }
}

/// The most specific level decides: a package policy wins over its category's and the global
/// one, and clearing it hands the decision back.
#[tokio::test]
async fn the_package_level_wins_and_clearing_it_inherits_again() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (scheduler, database, file, _) = colliding(temporary.path()).await;
    let global = scheduler
        .effective_collision_policy(file.package_id)
        .await
        .expect("effective");
    assert_eq!(global.policy, CollisionPolicy::Rename);
    assert_eq!(global.source, rd_core::CollisionPolicySource::Global);

    database
        .set_package_collision_policy(file.package_id, Some(CollisionPolicy::Skip))
        .await
        .expect("policy");
    let own = scheduler
        .effective_collision_policy(file.package_id)
        .await
        .expect("effective");
    assert_eq!(own.policy, CollisionPolicy::Skip);
    assert_eq!(own.source, rd_core::CollisionPolicySource::Package);

    database
        .set_package_collision_policy(file.package_id, None)
        .await
        .expect("clear");
    assert_eq!(
        scheduler
            .effective_collision_policy(file.package_id)
            .await
            .expect("effective")
            .source,
        rd_core::CollisionPolicySource::Global
    );
}

/// `ask` across a restart: the prompt and the block are rows, only this download waits, and
/// the answer given after the restart is the one carried out.
#[tokio::test]
async fn an_ask_survives_a_restart_and_its_answer_is_carried_out() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (scheduler, database, file, destination) = colliding(temporary.path()).await;
    database
        .set_package_collision_policy(file.package_id, Some(CollisionPolicy::Ask))
        .await
        .expect("policy");
    let part = destination.join(".rdownloader").join("x.part");
    let file = started(&database, &file).await;
    let flow = before_transfer(&scheduler, &file, &destination, &part, None)
        .await
        .expect("decision");
    assert_eq!(flow, ControlFlow::Break(()));
    drop(scheduler);

    // The restart.
    let (scheduler, database) = scheduler_over(temporary.path()).await;
    let row = state_of(&database, &file).await;
    assert_eq!(
        row.state,
        DownloadState::Blocked,
        "the restart released the question"
    );
    let prompt = database
        .collision_prompt(file.id)
        .await
        .expect("prompt")
        .expect("the prompt survived");
    assert_eq!(prompt.target_name, "file.bin");
    assert_eq!(prompt.existing_bytes, Some(8));

    // The answer, the way the endpoint gives it.
    assert!(
        database
            .decide_collision_prompt(file.id, CollisionDecision::Overwrite)
            .await
            .expect("decide")
    );
    // The endpoint requeues it; the next attempt reaches the policy in `Resolving`. Taken
    // there directly, so no supervisor dispatches the row while the case inspects it.
    database
        .transition_download(file.id, DownloadState::Resolving)
        .await
        .expect("resolving");
    let file = state_of(&database, &file).await;
    let flow = before_transfer(&scheduler, &file, &destination, &part, None)
        .await
        .expect("decision");
    assert_eq!(
        flow,
        ControlFlow::Continue(destination.join("file.bin")),
        "the answer, not the policy, decided the second attempt"
    );
}

/// A name taken while the transfer ran is not replaced by the promotion: the default policy
/// renames, as it always did before 1.5 for a name taken at the start.
#[tokio::test]
async fn a_name_taken_during_the_transfer_is_never_replaced_silently() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (scheduler, database, file, destination) = colliding(temporary.path()).await;
    let file = verifying(&database, &file).await;
    let part = destination.join(".rdownloader").join("x.part");
    tokio::fs::create_dir_all(part.parent().expect("staging"))
        .await
        .expect("staging");
    tokio::fs::write(&part, b"fetched").await.expect("part");

    let flow = after_transfer(&scheduler, &file, &part, destination.join("file.bin"), None)
        .await
        .expect("decision");

    let ControlFlow::Continue((path, overwrite)) = flow else {
        panic!("the default policy renames");
    };
    assert_eq!(path, destination.join("file (1).bin"));
    assert!(overwrite.is_none());
}

/// The guard `overwrite` asks: a file whose download is still working on it is in use, and a
/// download never guards against itself.
#[tokio::test]
async fn a_file_in_use_is_recognised_and_a_download_is_not_in_its_own_way() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (scheduler, database, file, destination) = colliding(temporary.path()).await;
    let target = destination.join("file.bin");
    assert!(
        !scheduler.file_in_use(&target, None).await.expect("guard"),
        "a paused download holds nothing"
    );
    let file = verifying(&database, &file).await;
    assert!(
        scheduler.file_in_use(&target, None).await.expect("guard"),
        "a verifying download holds its file"
    );
    assert!(
        !scheduler
            .file_in_use(&target, Some(file.id))
            .await
            .expect("guard"),
        "a download never guards against itself"
    );
}

/// `compare` after the transfer: identical bytes keep the existing file, drop the part and
/// index the result; the download completes without a second copy.
#[tokio::test]
async fn an_identical_file_is_adopted_after_the_transfer() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (scheduler, database, file, destination) = colliding(temporary.path()).await;
    database
        .set_package_collision_policy(file.package_id, Some(CollisionPolicy::Compare))
        .await
        .expect("policy");
    let file = verifying(&database, &file).await;
    let part = destination.join(".rdownloader").join("x.part");
    tokio::fs::create_dir_all(part.parent().expect("staging"))
        .await
        .expect("staging");
    tokio::fs::write(&part, b"existing").await.expect("part");
    let digest = rd_files::compute_checksum(&part, rd_core::ChecksumAlgorithm::Sha256)
        .await
        .expect("digest");
    let computed = ExpectedChecksum {
        algorithm: digest.algorithm,
        value: digest.value,
    };

    let flow = after_transfer(
        &scheduler,
        &file,
        &part,
        destination.join("file.bin"),
        Some(&computed),
    )
    .await
    .expect("decision");

    assert_eq!(flow, ControlFlow::Break(()));
    assert!(!part.exists(), "the redundant part file was kept");
    assert!(!destination.join("file (1).bin").exists());
    let row = state_of(&database, &file).await;
    assert_eq!(row.state, DownloadState::Completed);
    let entry = database
        .content_index_entry(file.id)
        .await
        .expect("index")
        .expect("indexed");
    assert_eq!(entry.digest, computed.value);
}

/// The index follows the disk: a deleted file is marked missing, and found again when it
/// comes back; nothing is forgotten because a file was away.
#[tokio::test]
async fn the_content_index_check_marks_missing_files_and_finds_them_again() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (scheduler, database, file, destination) = colliding(temporary.path()).await;
    let path = destination.join("file.bin");
    database
        .index_content(
            file.id,
            "sha256".to_owned(),
            "00".to_owned(),
            8,
            path.to_string_lossy().into_owned(),
        )
        .await
        .expect("index");

    tokio::fs::remove_file(&path).await.expect("remove");
    let report = scheduler.check_content_index().await.expect("check");
    assert_eq!(report.missing, 1);
    assert!(
        database
            .content_index_entry(file.id)
            .await
            .expect("read")
            .expect("kept")
            .missing_since
            .is_some()
    );

    tokio::fs::write(&path, b"existing").await.expect("back");
    let report = scheduler.check_content_index().await.expect("check");
    assert_eq!(report.missing, 0);
    // Found again. The count is not asserted: the start's own background check can run at any
    // point of this test and restore the entry first, which leaves this check nothing to count.
    assert!(
        database
            .content_index_entry(file.id)
            .await
            .expect("read")
            .expect("kept")
            .missing_since
            .is_none()
    );
}

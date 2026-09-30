//! Collision policies, `ask` prompts, the content index and the storage history survive a
//! restart and stay consistent with the rows they belong to (RD-150-01, RD-150-02); clearing
//! the history keeps what is running and clearing the index keeps the downloads (RD-180-13).

use rd_core::{
    CollisionDecision, CollisionPhase, CollisionPolicy, DownloadId, PackageId,
    StorageOperationKind, StorageOperationState,
};
use rd_db::{
    Database, NewCollisionPrompt, NewStorageOperation, STORAGE_OPERATIONS_KEPT,
    StorageOperationOutcome,
};
use tempfile::TempDir;

async fn open(directory: &TempDir) -> Database {
    Database::open(directory.path().join("collisions.sqlite"))
        .await
        .expect("database")
}

async fn download(database: &Database, directory: &TempDir) -> (PackageId, DownloadId) {
    let package_id = PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "release".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download_id = DownloadId::new();
    database
        .create_download(rd_db::NewDownload {
            id: download_id,
            package_id,
            source: "https://example.com/file.bin".parse().expect("URL"),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
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
    (package_id, download_id)
}

#[tokio::test]
async fn a_package_policy_is_stored_read_back_and_cleared() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let (package_id, _) = download(&database, &directory).await;

    let levels = database
        .collision_policy_levels(package_id)
        .await
        .expect("levels");
    assert_eq!(levels.package, None);
    assert_eq!(levels.category, None);

    database
        .set_package_collision_policy(package_id, Some(CollisionPolicy::Ask))
        .await
        .expect("set");
    let levels = database
        .collision_policy_levels(package_id)
        .await
        .expect("levels");
    assert_eq!(levels.package, Some(CollisionPolicy::Ask));
    assert_eq!(
        database
            .list_collision_policies()
            .await
            .expect("list")
            .len(),
        1
    );

    database
        .set_package_collision_policy(package_id, None)
        .await
        .expect("clear");
    let levels = database
        .collision_policy_levels(package_id)
        .await
        .expect("levels");
    assert_eq!(levels.package, None, "cleared means inherited again");
}

/// The restart half of `ask`: the question and its answer are rows, so a new process finds
/// both exactly where the old one left them.
#[tokio::test]
async fn an_open_prompt_and_its_answer_survive_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let download_id = {
        let database = open(&directory).await;
        let (_, download_id) = download(&database, &directory).await;
        database
            .open_collision_prompt(NewCollisionPrompt {
                download_id,
                target_name: "file.bin".to_owned(),
                phase: CollisionPhase::BeforeTransfer,
                existing_bytes: Some(42),
            })
            .await
            .expect("open");
        download_id
    };

    let database = open(&directory).await;
    let prompt = database
        .collision_prompt(download_id)
        .await
        .expect("read")
        .expect("the prompt survived");
    assert_eq!(prompt.target_name, "file.bin");
    assert_eq!(prompt.existing_bytes, Some(42));
    assert_eq!(prompt.decision, None);

    assert!(
        database
            .decide_collision_prompt(download_id, CollisionDecision::Overwrite)
            .await
            .expect("decide")
    );
    drop(database);
    let database = open(&directory).await;
    let prompt = database
        .collision_prompt(download_id)
        .await
        .expect("read")
        .expect("still there");
    assert_eq!(prompt.decision, Some(CollisionDecision::Overwrite));
    assert!(prompt.decided_at.is_some());

    // A new collision is a new question: the old answer does not carry over.
    database
        .open_collision_prompt(NewCollisionPrompt {
            download_id,
            target_name: "file.bin".to_owned(),
            phase: CollisionPhase::AfterTransfer,
            existing_bytes: None,
        })
        .await
        .expect("reopen");
    let prompt = database
        .collision_prompt(download_id)
        .await
        .expect("read")
        .expect("reopened");
    assert_eq!(prompt.decision, None);
    assert_eq!(prompt.phase, CollisionPhase::AfterTransfer);

    database
        .clear_collision_prompt(download_id)
        .await
        .expect("clear");
    assert!(
        database
            .collision_prompt(download_id)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        !database
            .decide_collision_prompt(download_id, CollisionDecision::Skip)
            .await
            .expect("decide"),
        "an answer without a question changes nothing"
    );
}

#[tokio::test]
async fn the_content_index_follows_moves_missing_files_and_removals() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let (_, first) = download(&database, &directory).await;
    let (_, second) = download(&database, &directory).await;
    for (id, path) in [(first, "/a/file.bin"), (second, "/b/file.bin")] {
        database
            .index_content(
                id,
                "sha256".to_owned(),
                "ABCDEF".to_owned(),
                3,
                path.to_owned(),
            )
            .await
            .expect("index");
    }
    let matches = database
        .content_index_matches("sha256", "abcdef")
        .await
        .expect("matches");
    assert_eq!(matches.len(), 2, "the digest is compared in lowercase");

    database
        .move_indexed_content(first, "/c/file.bin".to_owned())
        .await
        .expect("move");
    database
        .mark_indexed_content(vec![(second, true)])
        .await
        .expect("mark");
    let first_entry = database
        .content_index_entry(first)
        .await
        .expect("read")
        .expect("entry");
    assert_eq!(first_entry.path, "/c/file.bin");
    let second_entry = database
        .content_index_entry(second)
        .await
        .expect("read")
        .expect("entry");
    let missing_since = second_entry.missing_since.expect("marked missing");

    // Marking it missing again keeps the first time it was found missing.
    database
        .mark_indexed_content(vec![(second, true)])
        .await
        .expect("mark again");
    assert_eq!(
        database
            .content_index_entry(second)
            .await
            .expect("read")
            .expect("entry")
            .missing_since,
        Some(missing_since)
    );
    database
        .mark_indexed_content(vec![(second, false)])
        .await
        .expect("found again");
    assert!(
        database
            .content_index_entry(second)
            .await
            .expect("read")
            .expect("entry")
            .missing_since
            .is_none()
    );

    // An overwrite by `first` of the file `second` is indexed under drops `second`'s entry.
    database
        .move_indexed_content(first, "/b/file.bin".to_owned())
        .await
        .expect("move");
    assert_eq!(
        database
            .forget_indexed_path("/b/file.bin".to_owned(), first)
            .await
            .expect("forget"),
        1
    );
    assert!(
        database
            .content_index_entry(second)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        database
            .content_index_entry(first)
            .await
            .expect("read")
            .is_some()
    );

    database.delete_download(first).await.expect("remove");
    assert!(
        database
            .content_index_entry(first)
            .await
            .expect("read")
            .is_none(),
        "a removed download takes its index entry with it"
    );
}

#[tokio::test]
async fn a_running_operation_is_interrupted_by_a_restart_and_the_history_is_capped() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let operation = || NewStorageOperation {
        kind: StorageOperationKind::Move,
        package_id: None,
        download_id: None,
        source_path: "/old/file.bin".to_owned(),
        target_path: "/new/file.bin".to_owned(),
        size_bytes: Some(3),
    };
    let finished = database
        .start_storage_operation(operation())
        .await
        .expect("start");
    database
        .finish_storage_operation(
            finished,
            StorageOperationOutcome::completed(Some(3), Some("abc".to_owned())),
        )
        .await
        .expect("finish");
    let running = database
        .start_storage_operation(operation())
        .await
        .expect("start");
    drop(database);

    let database = open(&directory).await;
    assert_eq!(
        database
            .interrupt_storage_operations()
            .await
            .expect("interrupt"),
        1
    );
    let history = database.list_storage_operations(10).await.expect("list");
    assert_eq!(history[0].id, running, "newest first");
    assert_eq!(history[0].state, StorageOperationState::Interrupted);
    assert_eq!(history[1].state, StorageOperationState::Completed);
    assert_eq!(history[1].verified_digest.as_deref(), Some("abc"));

    // A late answer cannot rewrite what recovery settled.
    database
        .finish_storage_operation(
            running,
            StorageOperationOutcome::failed("storage.move_failed", "late".to_owned()),
        )
        .await
        .expect("late finish");
    let history = database.list_storage_operations(10).await.expect("list");
    assert_eq!(history[0].state, StorageOperationState::Interrupted);

    let cap = usize::try_from(STORAGE_OPERATIONS_KEPT).expect("cap");
    for _ in 0..cap {
        database
            .start_storage_operation(operation())
            .await
            .expect("start");
    }
    let everything = database
        .list_storage_operations(u32::MAX)
        .await
        .expect("list");
    assert_eq!(
        everything.len(),
        cap,
        "the oldest rows beyond the cap are gone"
    );
}

/// Clearing the history keeps what is still under way (RD-180-13): a running move records how
/// it ended into its own row, and a restart settles that row as `interrupted`.
#[tokio::test]
async fn clearing_the_history_keeps_a_running_operation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let operation = || NewStorageOperation {
        kind: StorageOperationKind::Dedupe,
        package_id: None,
        download_id: None,
        source_path: "/a/file.bin".to_owned(),
        target_path: "/b/file.bin".to_owned(),
        size_bytes: None,
    };
    for outcome in [
        StorageOperationOutcome::completed(Some(3), Some("abc".to_owned())),
        StorageOperationOutcome::failed("storage.move_failed", "gone".to_owned()),
    ] {
        let id = database
            .start_storage_operation(operation())
            .await
            .expect("start");
        database
            .finish_storage_operation(id, outcome)
            .await
            .expect("finish");
    }
    let running = database
        .start_storage_operation(operation())
        .await
        .expect("start");
    assert_eq!(
        database
            .count_clearable_storage_operations()
            .await
            .expect("count"),
        2,
        "the count names what a clear removes, not the whole table"
    );

    assert_eq!(database.clear_storage_operations().await.expect("clear"), 2);

    let history = database.list_storage_operations(10).await.expect("list");
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!(history[0].id, running);
    assert_eq!(history[0].state, StorageOperationState::Running);
    // The kept row still takes its outcome.
    database
        .finish_storage_operation(
            running,
            StorageOperationOutcome::completed(None, Some("abc".to_owned())),
        )
        .await
        .expect("finish");
    let history = database.list_storage_operations(10).await.expect("list");
    assert_eq!(history[0].state, StorageOperationState::Completed);
    assert_eq!(database.clear_storage_operations().await.expect("clear"), 1);
    assert_eq!(
        database.clear_storage_operations().await.expect("clear"),
        0,
        "an empty history is already what was asked for"
    );
}

/// Clearing the index drops every entry, missing ones included, and nothing else: the
/// downloads stay, and a new entry can be written right after (RD-180-13).
#[tokio::test]
async fn clearing_the_content_index_removes_every_entry_and_no_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let (_, first) = download(&database, &directory).await;
    let (_, second) = download(&database, &directory).await;
    for id in [first, second] {
        database
            .index_content(
                id,
                "sha256".to_owned(),
                "abcdef".to_owned(),
                3,
                format!("/data/{id}.bin"),
            )
            .await
            .expect("index");
    }
    database
        .mark_indexed_content(vec![(second, true)])
        .await
        .expect("mark");
    assert_eq!(database.count_content_index().await.expect("count"), 2);

    assert_eq!(database.clear_content_index().await.expect("clear"), 2);

    assert_eq!(database.count_content_index().await.expect("count"), 0);
    assert!(
        database
            .content_index_matches("sha256", "abcdef")
            .await
            .expect("matches")
            .is_empty()
    );
    for id in [first, second] {
        assert!(
            database.get_download(id).await.expect("read").is_some(),
            "the download went with its index entry"
        );
    }
    database
        .index_content(
            first,
            "sha256".to_owned(),
            "abcdef".to_owned(),
            3,
            "/data/again.bin".to_owned(),
        )
        .await
        .expect("index again");
    assert_eq!(database.count_content_index().await.expect("count"), 1);
}

//! Audit 1.9.1 — what one dispatch pass reads and does (TR-08, TR-18, RA-TR-07).

use std::{path::Path, sync::atomic::Ordering};

use rd_core::{DownloadKind, DownloadState};

use crate::{BlockReason, FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("dispatch.sqlite3"))
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

fn spec(directory: &Path, start_paused: bool) -> PackageSpec {
    PackageSpec {
        name: "release".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    }
}

fn file(name: &str, kind: DownloadKind) -> FileSpec {
    FileSpec {
        source: format!("ftp://files.example/{name}").parse().expect("url"),
        file_name: name.to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::default(),
        kind,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }
}

/// TR-08: the dispatcher's read is the startable rows only, and the budget's odometer is a sum
/// SQLite takes, not a fold over the whole table.
#[tokio::test]
async fn the_dispatcher_reads_only_what_can_start() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let (_, files) = scheduler
        .enqueue_package(
            spec(directory.path(), true),
            vec![
                file("a.bin", DownloadKind::Ftp),
                file("b.bin", DownloadKind::Ftp),
                file("c.bin", DownloadKind::Ftp),
            ],
        )
        .await
        .expect("enqueue");
    database
        .transition_download(files[1].id, DownloadState::Queued)
        .await
        .expect("queue one");
    database
        .set_download_progress(files[0].id, 300, None)
        .await
        .expect("progress");
    database
        .set_download_progress(files[2].id, 45, None)
        .await
        .expect("progress");

    let startable = database.startable_downloads().await.expect("startable");
    assert_eq!(
        startable.iter().map(|file| file.id).collect::<Vec<_>>(),
        [files[1].id],
        "paused rows are no business of the dispatcher"
    );
    assert_eq!(database.committed_bytes_total().await.expect("sum"), 345);

    // And the pass itself reads through that query, once. The supervisor is stopped first so
    // the count is this pass's alone; its token check comes after the read.
    scheduler.shutdown().await.expect("shutdown");
    let before = scheduler.queue_reads.load(Ordering::Acquire);
    scheduler.schedule_runnable().await.expect("pass");
    assert_eq!(
        scheduler.queue_reads.load(Ordering::Acquire) - before,
        1,
        "a dispatch pass reads the startable rows once and nothing else of the queue"
    );
}

/// TR-18: every waiting file of a switched-off kind is blocked by the first one the pass meets,
/// and none of them starts.
#[tokio::test]
async fn a_disabled_kind_is_blocked_in_one_pass() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    scheduler
        .disabled_kinds
        .lock()
        .await
        .push(DownloadKind::Ftp);
    // No supervisor passes beside the one below: the end state and the count are its own
    // (RA-TR-07). A shut-down scheduler still blocks; only the start is refused.
    scheduler.shutdown().await.expect("shutdown");
    let (_, files) = scheduler
        .enqueue_package(
            spec(directory.path(), false),
            vec![
                file("a.bin", DownloadKind::Ftp),
                file("b.bin", DownloadKind::Ftp),
                file("c.bin", DownloadKind::Ftp),
            ],
        )
        .await
        .expect("enqueue");

    let before = scheduler.queue_reads.load(Ordering::Acquire);
    scheduler.schedule_runnable().await.expect("pass");
    assert_eq!(
        scheduler.queue_reads.load(Ordering::Acquire) - before,
        2,
        "the pass's own read and one for the switched-off kind, not one per waiting file"
    );

    let blocked = database
        .downloads_blocked_by(BlockReason::KindDisabled.as_str())
        .await
        .expect("blocked");
    for file in &files {
        assert!(blocked.contains(&file.id), "{} still waits", file.file_name);
    }
    assert!(scheduler.active.lock().await.tokens.is_empty());
}

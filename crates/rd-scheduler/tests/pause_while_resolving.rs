//! A file paused while its source has not answered yet stays paused (RD-1130-04).
//!
//! An HTTP file stays `Resolving` until its source answers the probe. A pause in that phase
//! cancelled the worker, which then asked for `Resolving -> Paused` — an edge the state machine
//! did not have. The refusal ended the attempt as an error, the error was recorded as a failed
//! attempt, and the file went to `RetryWait` and started again a little later. The source is a
//! local listener that accepts and never answers, so the file is still resolving when the pause
//! reaches it.

use std::{path::Path, time::Duration};

use rd_core::{DownloadFile, DownloadId, DownloadState};
use rd_scheduler::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};
use tokio::net::TcpListener;

/// Accepts every connection and holds it open without a byte of answer.
async fn silent_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    port
}

async fn start(directory: &Path, database: &rd_db::Database) -> SchedulerHandle {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: 1,
            ..SchedulerConfig::for_directory(directory.join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler")
}

async fn file(scheduler: &SchedulerHandle, directory: &Path, port: u16) -> DownloadFile {
    let spec = PackageSpec {
        name: format!("package {}", DownloadId::new()),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: false,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source: format!("http://127.0.0.1:{port}/{}.bin", DownloadId::new())
            .parse()
            .expect("url"),
        file_name: format!("{}.bin", DownloadId::new()),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
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
    let (_, files) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    files.into_iter().next().expect("one file")
}

async fn row(database: &rd_db::Database, id: DownloadId) -> DownloadFile {
    database.get_download(id).await.expect("read").expect("row")
}

/// Waits for `id` to reach `wanted`, at most `polls` times 100 ms; answers the last state seen.
async fn until_state(
    database: &rd_db::Database,
    id: DownloadId,
    wanted: DownloadState,
    polls: usize,
) -> DownloadState {
    let mut seen = row(database, id).await.state;
    for _ in 0..polls {
        if seen == wanted {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        seen = row(database, id).await.state;
    }
    seen
}

#[tokio::test]
async fn a_download_paused_while_resolving_stays_paused_and_counts_no_attempt() {
    let directory = tempfile::tempdir().expect("tempdir");
    let port = silent_listener().await;
    let database = rd_db::Database::open(directory.path().join("pause-resolving.sqlite3"))
        .await
        .expect("database");
    let scheduler = start(directory.path(), &database).await;
    let resolving = file(&scheduler, directory.path(), port).await;

    // The dispatch pass runs twice a second; the probe never gets an answer, so the file stays
    // `Resolving` once it started.
    let started = until_state(&database, resolving.id, DownloadState::Resolving, 100).await;
    assert_eq!(
        started,
        DownloadState::Resolving,
        "the file has to be resolving when it is paused"
    );

    scheduler.pause(resolving.id).await.expect("pause");
    let stopped = until_state(&database, resolving.id, DownloadState::Paused, 50).await;
    assert_eq!(
        stopped,
        DownloadState::Paused,
        "a pause while resolving did not end as a pause"
    );

    // Longer than a retry's wait and a few dispatch passes: nothing starts it again.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let held = row(&database, resolving.id).await;
    assert_eq!(
        held.state,
        DownloadState::Paused,
        "the paused file started again"
    );
    assert_eq!(
        held.retry_count, 0,
        "the pause was counted as a failed attempt"
    );
    assert!(
        held.last_error.is_none(),
        "the pause was recorded as an error: {:?}",
        held.last_error
    );

    // A resume sets it going again.
    scheduler.resume(resolving.id).await.expect("resume");
    let resumed = row(&database, resolving.id).await.state;
    assert!(
        matches!(resumed, DownloadState::Queued | DownloadState::Resolving),
        "a resume did not put the file back into the queue: {resumed:?}"
    );
    scheduler.shutdown().await.expect("shutdown");
}

//! A download that runs when the service stops runs again after the next start.
//!
//! The service's own stop — an update, a restart, the tray's quit — cancels every running file
//! without a reason of the user's. Those files used to be written `Paused`, which the next
//! start leaves alone, so every update paused every running download until somebody resumed it
//! by hand (the 1.12.0 release candidate's self-update run, 2026-10-06). A pause the user asked
//! for must still stay a pause. The source is a local listener that accepts and never answers,
//! so a started file stays started for as long as the case runs.

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
            // One slot: the second file waits, and is paused while it waits.
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
        // One name per file: two files of the same name in one folder would collide.
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

async fn state(database: &rd_db::Database, id: DownloadId) -> DownloadState {
    database
        .get_download(id)
        .await
        .expect("read")
        .expect("row")
        .state
}

/// Waits for the dispatch pass, which runs twice a second, to have started `id`.
async fn until_started(database: &rd_db::Database, id: DownloadId) {
    for _ in 0..100 {
        if state(database, id).await != DownloadState::Queued {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{id} never started");
}

#[tokio::test]
async fn a_running_download_is_queued_again_after_a_shutdown_and_a_pause_stays_a_pause() {
    let directory = tempfile::tempdir().expect("tempdir");
    let port = silent_listener().await;
    let database = rd_db::Database::open(directory.path().join("shutdown.sqlite3"))
        .await
        .expect("database");
    let scheduler = start(directory.path(), &database).await;
    let running = file(&scheduler, directory.path(), port).await;
    let paused = file(&scheduler, directory.path(), port).await;
    until_started(&database, running.id).await;
    scheduler.pause(paused.id).await.expect("pause");
    let before = state(&database, running.id).await;
    assert!(
        matches!(
            before,
            DownloadState::Resolving | DownloadState::Downloading
        ),
        "the file has to be running when the service stops, not {before:?}"
    );

    scheduler.shutdown().await.expect("shutdown");
    assert_ne!(
        state(&database, running.id).await,
        DownloadState::Paused,
        "the service's own stop paused a running download"
    );
    assert_eq!(state(&database, paused.id).await, DownloadState::Paused);

    // The next start: the interrupted file is queued again, the paused one stays paused.
    let restarted = start(directory.path(), &database).await;
    assert!(matches!(
        state(&database, running.id).await,
        DownloadState::Queued | DownloadState::Resolving | DownloadState::Downloading
    ));
    assert_eq!(state(&database, paused.id).await, DownloadState::Paused);
    restarted.shutdown().await.expect("shutdown");
}

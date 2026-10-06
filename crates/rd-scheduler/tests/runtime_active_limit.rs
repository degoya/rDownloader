//! RD-1120-22 — the number of parallel downloads changes while the service runs.
//!
//! The status bar sets `max_active_files` through the settings route, which hands it to
//! `update_runtime_settings`; the dispatch pass reads it on every tick. These cases hold that
//! a raised limit starts what waits, and that the new value binds rather than the old one,
//! without a restart. The source is a local listener that accepts and never answers, so a
//! started file stays started for as long as the case runs.

use std::{path::Path, time::Duration};

use rd_core::{DownloadFile, DownloadId, DownloadState};
use rd_scheduler::{FileSpec, PackageSpec, RuntimeSettings, SchedulerConfig, SchedulerHandle};
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

async fn installation(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("active-limit.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: 0,
            ..SchedulerConfig::for_directory(directory.join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    (scheduler, database)
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
        file_name: "file.bin".to_owned(),
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

/// How many of the files the dispatch pass has started.
async fn started(database: &rd_db::Database, ids: &[DownloadId]) -> usize {
    let mut count = 0;
    for id in ids {
        if state(database, *id).await != DownloadState::Queued {
            count += 1;
        }
    }
    count
}

/// Waits for the dispatch pass, which runs twice a second, to have started `wanted` files.
async fn until_started(database: &rd_db::Database, ids: &[DownloadId], wanted: usize) {
    for _ in 0..100 {
        if started(database, ids).await >= wanted {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("never {wanted} of {ids:?} started");
}

async fn set_limit(scheduler: &SchedulerHandle, max_active_files: usize) {
    scheduler
        .update_runtime_settings(RuntimeSettings {
            max_active_files,
            ..RuntimeSettings::default()
        })
        .await
        .expect("settings");
}

#[tokio::test]
async fn a_changed_limit_applies_to_the_running_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let port = silent_listener().await;
    let (scheduler, database) = installation(directory.path()).await;
    let first = file(&scheduler, directory.path(), port).await;
    let second = file(&scheduler, directory.path(), port).await;
    let ids = [first.id, second.id];

    // Three ticks with no slot at all: nothing starts.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(started(&database, &ids).await, 0);

    set_limit(&scheduler, 1).await;
    until_started(&database, &ids, 1).await;
    // The new value binds: one slot, held by whichever file took it.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(started(&database, &ids).await, 1);

    set_limit(&scheduler, 2).await;
    until_started(&database, &ids, 2).await;

    scheduler.shutdown().await.expect("shutdown");
}

#[test]
fn a_limit_of_zero_is_refused() {
    let settings = RuntimeSettings {
        max_active_files: 0,
        ..RuntimeSettings::default()
    };
    assert!(SchedulerHandle::validate_runtime_settings(&settings).is_err());
}

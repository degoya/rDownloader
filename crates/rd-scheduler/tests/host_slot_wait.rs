//! RD-1130-02 — a file that would only wait for a connection to its host leaves its place to a
//! file of another host.
//!
//! Each file plans four chunks and a host allows six connections, so two files of one host
//! fill it. The third used to be started all the same: it took one of the `max_active_files`
//! places and then waited inside the engine, showing *Downloading* with 0 B, while a file of
//! another host stayed queued behind it. The source is a local listener that accepts and never
//! answers, so a started file stays started for as long as the case runs; `localhost` and
//! `127.0.0.1` are two hosts to the limit, though one listener serves both.

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

async fn file(
    scheduler: &SchedulerHandle,
    directory: &Path,
    host: &str,
    port: u16,
) -> DownloadFile {
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
        source: format!("http://{host}:{port}/{}.bin", DownloadId::new())
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
async fn a_file_waiting_for_its_host_leaves_its_place_to_another_host() {
    let directory = tempfile::tempdir().expect("tempdir");
    let port = silent_listener().await;
    let database = rd_db::Database::open(directory.path().join("host-wait.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: 3,
            ..SchedulerConfig::for_directory(directory.path().join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    // Queue order: the three files of one host first, so the old dispatch filled all three
    // places with them and never reached the fourth.
    let mut same_host = Vec::new();
    for _ in 0..3 {
        same_host.push(file(&scheduler, directory.path(), "127.0.0.1", port).await);
    }
    let other_host = file(&scheduler, directory.path(), "localhost", port).await;

    until_started(&database, other_host.id).await;
    // A few more passes: the waiting file must not start behind the other host's either.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let mut waiting = Vec::new();
    for download in &same_host {
        if state(&database, download.id).await == DownloadState::Queued {
            waiting.push(download.id);
        }
    }
    assert_eq!(
        waiting.len(),
        1,
        "two files fill the host's six connections, the third waits"
    );
    let waits = scheduler.host_waits().await;
    assert_eq!(
        waits.get(&waiting[0]).map(String::as_str),
        Some("127.0.0.1"),
        "the waiting file says which host it waits for: {waits:?}"
    );
    assert!(
        !waits.contains_key(&other_host.id),
        "a started file waits for nothing"
    );

    scheduler.shutdown().await.expect("shutdown");
}

//! RD-1130-02 — a file that would only wait for a connection to its host leaves its place to a
//! file of another host; RD-1140-07 — and waits only when its host has no connection left.
//!
//! A starting file promises its host the connections it will open there: one per file of one
//! chunk, its chunks for a direct link split into several (RD-1240-33). A host that allows two
//! holds the third of its one-chunk files back; that file used to be started all the same: it took one of the
//! `max_active_files` places and then waited inside the engine, showing *Downloading* with
//! 0 B, while a file of another host stayed queued behind it. The source is a local listener
//! that accepts and never answers, so a started file stays started for as long as the case
//! runs; `localhost` and `127.0.0.1` are two hosts to the limit, though one listener serves
//! both.

use std::{path::Path, time::Duration};

use rd_core::{DownloadFile, DownloadId, DownloadState};
use rd_scheduler::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

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

/// A hoster: `127.0.0.1` is its page and answers every request with a redirect to the same
/// path on `localhost`, its download server, which answers the probe's `HEAD` and never a
/// byte of the transfer.
async fn hoster_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => raw.extend_from_slice(&buffer[..read]),
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                let path = text
                    .lines()
                    .next()
                    .and_then(|line| line.split(' ').nth(1))
                    .unwrap_or("/")
                    .to_owned();
                let on_the_page = text.lines().any(|line| {
                    line.split_once(':').is_some_and(|(key, value)| {
                        key.trim().eq_ignore_ascii_case("host")
                            && value.trim().starts_with("127.0.0.1")
                    })
                });
                if on_the_page {
                    let redirect = format!(
                        "HTTP/1.1 302 Found\r\nlocation: http://localhost:{port}{path}\r\n\
                         content-length: 0\r\nconnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(redirect.as_bytes()).await;
                    let _ = stream.shutdown().await;
                    return;
                }
                // The download server: the probe learns the size and where the redirect led,
                // the chunks are held open without a byte.
                if text.starts_with("HEAD ") {
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                                content-length: 8388608\r\naccept-ranges: bytes\r\n\
                                connection: close\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.shutdown().await;
                    return;
                }
                let _held = stream;
                std::future::pending::<()>().await;
            });
        }
    });
    port
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
            // One connection per file: two take the host's two (the chunked case below).
            max_chunks_per_file: 1,
            max_connections_per_host: 2,
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
        "two starting files take the host's two connections, the third waits"
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

/// RD-1140-07 — a hoster's files run side by side. Each file of the hoster page promised its
/// host as many connections as it planned chunks, six of six here, and the bytes came from
/// another host, so nothing ever took the promise back: one file ran, the others waited
/// (1.13.0, DDownload). A direct link promises its chunks again since RD-1240-33, so the next
/// file starts once the probe has followed the redirect to the download server, which ends
/// the promise to the page's host; a download server that never answered even the probe
/// would hold it for `START_GRACE`.
#[tokio::test]
async fn files_of_a_hoster_downloaded_from_another_host_run_side_by_side() {
    let directory = tempfile::tempdir().expect("tempdir");
    let port = hoster_listener().await;
    let database = rd_db::Database::open(directory.path().join("hoster-wait.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: 3,
            max_chunks_per_file: 6,
            max_connections_per_host: 6,
            ..SchedulerConfig::for_directory(directory.path().join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let mut files = Vec::new();
    for _ in 0..3 {
        files.push(file(&scheduler, directory.path(), "127.0.0.1", port).await);
    }

    let mut states = Vec::new();
    for _ in 0..50 {
        states.clear();
        for download in &files {
            states.push(state(&database, download.id).await);
        }
        if states.iter().all(|state| *state != DownloadState::Queued) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        states.iter().all(|state| state.is_working()),
        "three files of one hoster run at once: {states:?}"
    );
    let waits = scheduler.host_waits().await;
    assert!(waits.is_empty(), "no file waits for its host: {waits:?}");

    scheduler.shutdown().await.expect("shutdown");
}

/// RD-1240-33, the live finding: parallel 3, six connections per host, four chunks per file;
/// three big files of one host and one of another. Two files of the first host take its six
/// connections (four and two); the third used to be admitted all the same, waited inside the
/// engine on one of the three places and kept the other host's file queued, with no hint.
#[tokio::test]
async fn a_chunked_file_that_would_only_wait_leaves_its_place_to_another_host() {
    let directory = tempfile::tempdir().expect("tempdir");
    let port = silent_listener().await;
    let database = rd_db::Database::open(directory.path().join("host-wait-chunks.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: 3,
            max_chunks_per_file: 4,
            max_connections_per_host: 6,
            ..SchedulerConfig::for_directory(directory.path().join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let mut same_host = Vec::new();
    for _ in 0..3 {
        same_host.push(file(&scheduler, directory.path(), "127.0.0.1", port).await);
    }
    let other_host = file(&scheduler, directory.path(), "localhost", port).await;

    until_started(&database, other_host.id).await;
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
        "two files of four chunks take the host's six connections, the third waits"
    );
    let waits = scheduler.host_waits().await;
    assert_eq!(
        waits.get(&waiting[0]).map(String::as_str),
        Some("127.0.0.1"),
        "the waiting file says which host it waits for: {waits:?}"
    );

    scheduler.shutdown().await.expect("shutdown");
}

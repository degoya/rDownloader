//! RD-150-03 — what a Metalink document may make the service do, end to end in the queue.
//!
//! A Metalink is written by whoever serves it. Its mirrors are addresses the service requests
//! on the person's behalf, and its file names become paths on the person's disk. These cases
//! run a hostile document's set through the real worker: every mirror that points at this
//! machine — or, for a set that did not come from the person's own hand, into their network —
//! is isolated with `mirror.internal_address` and never receives a single connection, the
//! download does not fall back to fetching its own address when that address is one of them,
//! a proposed link without mirrors is refused the same way on the single-source path, and
//! every hostile file name is flattened into one plain name inside the package folder.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rd_core::{DownloadFile, DownloadState, SourceSet};
use rd_scheduler::{FileSpec, PackageSpec, RuntimeSettings, SchedulerConfig, SchedulerHandle};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// A plain HTTP listener that serves 1 KiB to anyone and counts every connection it accepts.
async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let reached = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reached);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0_u8; 2048];
            let _ = stream.read(&mut buffer).await;
            let mut answer = b"HTTP/1.1 200 OK\r\ncontent-length: 1024\r\naccept-ranges: bytes\r\n\
                content-type: application/octet-stream\r\nconnection: close\r\n\r\n"
                .to_vec();
            answer.extend_from_slice(&[7_u8; 1024]);
            let _ = stream.write_all(&answer).await;
        }
    });
    (port, reached)
}

async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("metalink-bounds.sqlite3"))
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
    scheduler
        .update_runtime_settings(RuntimeSettings {
            max_retries: 0,
            ..RuntimeSettings::default()
        })
        .await
        .expect("settings");
    (scheduler, database)
}

fn spec(directory: &Path, start_paused: bool) -> PackageSpec {
    PackageSpec {
        name: "Metalink".to_owned(),
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

fn file(set: SourceSet, file_name: &str) -> FileSpec {
    FileSpec {
        source: set.sources[0].url.clone(),
        file_name: file_name.to_owned(),
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
        source_set: Some(Box::new(set)),
    }
}

fn set(addresses: &[String], size: Option<u64>) -> SourceSet {
    SourceSet::checked(
        addresses
            .iter()
            .map(|address| (address.clone(), None, None)),
        size,
        &[],
        None,
    )
    .expect("set")
}

/// Runs one download and waits until it finished or failed.
async fn run(
    scheduler: &SchedulerHandle,
    database: &rd_db::Database,
    directory: &Path,
    set: SourceSet,
) -> DownloadFile {
    let (_, files) = scheduler
        .enqueue_package(spec(directory, false), vec![file(set, "f.bin")])
        .await
        .expect("enqueue");
    let id = files.first().expect("one file").id;
    for _ in 0..300 {
        let current = database
            .get_download(id)
            .await
            .expect("read")
            .expect("download");
        if current.state == DownloadState::Completed || current.last_error.is_some() {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the download neither finished nor failed");
}

#[tokio::test]
async fn mirrors_that_point_inside_are_isolated_and_never_requested() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (port, reached) = counting_listener().await;
    let (scheduler, database) = scheduler_over(directory.path()).await;
    // A set relayed from elsewhere (the strict default): this machine in four spellings, the
    // cloud metadata endpoint, and a LAN address it may not reach either.
    let hostile = set(
        &[
            format!("http://127.0.0.1:{port}/f.bin"),
            format!("http://localhost:{port}/f.bin"),
            format!("http://[::1]:{port}/f.bin"),
            format!("http://[::ffff:127.0.0.1]:{port}/f.bin"),
            "http://169.254.169.254/latest/meta-data/f.bin".to_owned(),
            format!("http://192.168.0.1:{port}/f.bin"),
        ],
        Some(1024),
    );
    assert!(!hostile.local_network);
    let count = hostile.sources.len();

    let finished = run(&scheduler, &database, directory.path(), hostile).await;
    let error = finished.last_error.expect("failed");
    assert_eq!(
        error.code.as_deref(),
        Some(rd_core::CODE_NO_USABLE_SOURCE),
        "{error:?}"
    );
    let sources = database
        .download_sources(finished.id)
        .await
        .expect("sources");
    assert_eq!(sources.len(), count);
    for source in &sources {
        assert_eq!(
            source.isolated_code.as_deref(),
            Some(rd_core::CODE_INTERNAL_ADDRESS),
            "{}",
            source.url
        );
    }
    assert_eq!(
        reached.load(Ordering::SeqCst),
        0,
        "a refused mirror was requested"
    );
}

/// Without a size anywhere the worker hands the download to the single-source path, which
/// fetches the download's own address — here one of the refused mirrors. It must not.
#[tokio::test]
async fn a_set_without_a_size_does_not_fall_back_to_an_internal_address() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (port, reached) = counting_listener().await;
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let hostile = set(&[format!("http://127.0.0.1:{port}/f.bin")], None);

    let finished = run(&scheduler, &database, directory.path(), hostile).await;
    let error = finished.last_error.expect("failed");
    assert_eq!(
        error.code.as_deref(),
        Some(rd_core::CODE_INTERNAL_ADDRESS),
        "{error:?}"
    );
    assert_eq!(
        reached.load(Ordering::SeqCst),
        0,
        "the fallback requested it"
    );
}

/// A link a document or a page proposed without mirrors is queued with its own address as its
/// one source row (RD-150-03). The single-source path fetches it, held to the row's rule: this
/// machine is refused under its literal address and under a name that resolves to it — even
/// for a document the person handed over — and never receives a connection.
#[tokio::test]
async fn a_proposed_link_without_mirrors_is_never_requested_at_this_machine() {
    for host in ["127.0.0.1", "localhost"] {
        let directory = tempfile::tempdir().expect("tempdir");
        let (port, reached) = counting_listener().await;
        let (scheduler, database) = scheduler_over(directory.path()).await;
        let link: url::Url = format!("http://{host}:{port}/f.bin").parse().expect("url");
        let proposed = SourceSet::of_link(&link, true).expect("set");

        let finished = run(&scheduler, &database, directory.path(), proposed).await;
        let error = finished.last_error.expect("failed");
        assert_eq!(
            error.code.as_deref(),
            Some(rd_core::CODE_INTERNAL_ADDRESS),
            "{host}: {error:?}"
        );
        assert_eq!(reached.load(Ordering::SeqCst), 0, "{host} was requested");
    }
}

/// The file name is the document's too. Whatever it says, the queue keeps one plain name that
/// stays inside the package folder.
#[tokio::test]
async fn a_metalinks_file_names_cannot_leave_the_package_folder() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let hostile = [
        "../x",
        "../../etc/passwd",
        "/etc/x",
        "C:\\x",
        "C:x",
        "\\\\server\\share\\x",
        "a\0b\u{7}c\u{1b}[31m.bin",
        "..",
    ];
    let files = hostile
        .iter()
        .enumerate()
        .map(|(index, name)| {
            file(
                set(
                    &[format!("https://mirror{index}.example/f.bin")],
                    Some(1024),
                ),
                name,
            )
        })
        .collect();
    let (_, created) = scheduler
        .enqueue_package(spec(directory.path(), true), files)
        .await
        .expect("enqueue");
    assert_eq!(created.len(), hostile.len());
    for (download, written) in created.iter().zip(hostile) {
        let stored = database
            .get_download(download.id)
            .await
            .expect("read")
            .expect("download");
        let name = stored.file_name;
        assert!(
            !name.is_empty()
                && name != "."
                && name != ".."
                && !name.contains(['/', '\\', ':'])
                && !name.chars().any(char::is_control),
            "{written:?} became {name:?}"
        );
        // Joined to the package folder, it is one component below it and nothing else.
        let folder = directory.path().join("storage").join("Metalink");
        let path = folder.join(&name);
        assert_eq!(
            path.parent(),
            Some(folder.as_path()),
            "{written:?} became {name:?}"
        );
    }
}

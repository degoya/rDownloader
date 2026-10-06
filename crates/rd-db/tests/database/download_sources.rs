//! The sources of one download, their health and the chunk marks built on them (RD-150-03).
//!
//! What is proven here is the half of "order and failover are stable after a restart" that
//! lives in the database: the order written with the row is the order read back after the
//! store is closed and opened again, a failure's backoff and an isolation survive it, and a
//! chunk a hash refused can be moved back — and only back, and only by that path.

use rd_core::{
    ChecksumAlgorithm, ChunkId, DownloadId, ExpectedChecksum, SourceOutcome, SourceProtocol,
    SourceSet, StatedHash,
};
use rd_db::{Database, PersistedChunk};

async fn open(directory: &std::path::Path) -> Database {
    Database::open(directory.join("queue.sqlite3"))
        .await
        .expect("database")
}

fn metalink_set() -> SourceSet {
    SourceSet::checked(
        [
            (
                "https://second.example/f.iso".to_owned(),
                Some(2),
                Some("fr".to_owned()),
            ),
            (
                "https://first.example/f.iso".to_owned(),
                Some(1),
                Some("de".to_owned()),
            ),
            ("ftp://third.example/f.iso".to_owned(), None, None),
        ],
        Some(32 * 1024),
        &[StatedHash {
            algorithm: "sha-256".to_owned(),
            value: "ab".repeat(32),
        }],
        Some((
            "sha-1".to_owned(),
            16 * 1024,
            vec!["0".repeat(40), "1".repeat(40)],
        )),
    )
    .expect("set")
}

async fn download_with_sources(database: &Database, directory: &std::path::Path) -> DownloadId {
    download_with_set(database, directory, metalink_set()).await
}

async fn download_with_set(
    database: &Database,
    directory: &std::path::Path,
    set: SourceSet,
) -> DownloadId {
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "metalink".to_owned(),
            destination: directory.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .create_download_with_sources(
            rd_db::NewDownload {
                id: DownloadId::new(),
                package_id,
                source: set.sources[0].url.clone(),
                file_name: "f.iso".to_owned(),
                total_bytes: None,
                expected_checksum: set.checksum.clone(),
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
            },
            set,
        )
        .await
        .expect("download")
        .id
}

#[tokio::test]
async fn the_order_and_the_health_of_sources_survive_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let id = {
        let database = open(directory.path()).await;
        let id = download_with_sources(&database, directory.path()).await;
        database
            .record_source_outcome(
                id,
                0,
                SourceOutcome::Failed {
                    code: "download.http_status".to_owned(),
                    retry_after_seconds: None,
                },
            )
            .await
            .expect("failure");
        database
            .record_source_outcome(
                id,
                1,
                SourceOutcome::Isolated {
                    code: rd_core::CODE_PIECE_MISMATCH.to_owned(),
                },
            )
            .await
            .expect("isolation");
        // A delivery after the isolation must not bring the source back.
        database
            .record_source_outcome(id, 1, SourceOutcome::Delivered { bytes: 10 })
            .await
            .expect("delivery");
        id
    };

    let database = open(directory.path()).await;
    let sources = database.download_sources(id).await.expect("sources");
    let order: Vec<_> = sources
        .iter()
        .map(|source| (source.position, source.url.as_str(), source.protocol))
        .collect();
    assert_eq!(
        order,
        [
            (0, "https://first.example/f.iso", SourceProtocol::Https),
            (1, "https://second.example/f.iso", SourceProtocol::Https),
            (2, "ftp://third.example/f.iso", SourceProtocol::Ftp),
        ]
    );
    assert_eq!(sources[0].failures, 1);
    assert!(sources[0].backoff_until.is_some());
    assert_eq!(
        sources[0].last_error_code.as_deref(),
        Some("download.http_status")
    );
    assert_eq!(
        sources[1].isolated_code.as_deref(),
        Some(rd_core::CODE_PIECE_MISMATCH)
    );
    assert_eq!(sources[1].delivered_bytes, 10);
    assert_eq!(sources[0].location.as_deref(), Some("de"));

    let pieces = database
        .download_piece_hashes(id)
        .await
        .expect("pieces")
        .expect("stated");
    assert_eq!(pieces.algorithm, ChecksumAlgorithm::Sha1);
    assert_eq!(pieces.length, 16 * 1024);
    assert_eq!(pieces.hashes.len(), 2);
    let file = database.get_download(id).await.expect("row").expect("row");
    assert_eq!(
        file.expected_checksum,
        Some(ExpectedChecksum {
            algorithm: ChecksumAlgorithm::Sha256,
            value: "ab".repeat(32),
        })
    );
}

#[tokio::test]
async fn a_refused_chunk_goes_back_and_loses_its_verification() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let id = download_with_sources(&database, directory.path()).await;
    let first = ChunkId::new();
    let second = ChunkId::new();
    database
        .prepare_transfer(
            id,
            Some(32 * 1024),
            None,
            None,
            vec![
                PersistedChunk {
                    id: first,
                    start: 0,
                    end: Some(16 * 1024),
                    committed: 0,
                },
                PersistedChunk {
                    id: second,
                    start: 16 * 1024,
                    end: Some(32 * 1024),
                    committed: 16 * 1024,
                },
            ],
        )
        .await
        .expect("plan");
    database
        .checkpoint_chunk(first, 16 * 1024)
        .await
        .expect("checkpoint");
    database
        .mark_chunk(first, Some(1), true)
        .await
        .expect("mark");
    let marks = database.chunk_marks(id).await.expect("marks");
    assert_eq!(marks[0].source_position, Some(1));
    assert!(marks[0].verified);

    // An ordinary checkpoint never goes backwards; only the rewind does.
    assert!(database.checkpoint_chunk(first, 0).await.is_err());
    database.rewind_chunk(first, 0).await.expect("rewind");
    // And never past the start or beyond what was confirmed.
    assert!(database.rewind_chunk(second, 0).await.is_err());

    let transfer = database.load_transfer(id).await.expect("transfer");
    assert_eq!(transfer.chunks[0].committed, 0);
    let marks = database.chunk_marks(id).await.expect("marks");
    assert!(!marks[0].verified);
    assert_eq!(marks[0].source_position, Some(1));
    let file = database.get_download(id).await.expect("row").expect("row");
    assert_eq!(file.committed_bytes.get(), 0);
}

#[tokio::test]
async fn a_candidate_keeps_its_set_until_it_is_queued() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (_, _, candidates) = database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None],
            urls: vec!["https://first.example/f.iso".parse().expect("url")],
            file_names: vec![Some("f.iso".to_owned())],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let candidate = candidates[0].id;
    assert_eq!(
        database
            .candidate_source_set(candidate)
            .await
            .expect("read"),
        None
    );
    database
        .set_candidate_source_set(candidate, metalink_set())
        .await
        .expect("store");
    assert_eq!(
        database
            .candidate_source_set(candidate)
            .await
            .expect("read"),
        Some(metalink_set())
    );
}

/// Whether a set may reach the person's own network is decided once, at intake, and the
/// transfer reads it from every source row; a set that never said so stays on public addresses.
#[tokio::test]
async fn the_network_a_set_may_reach_is_written_with_its_sources() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let strict = download_with_sources(&database, directory.path()).await;
    let mut pasted = metalink_set();
    pasted.local_network = true;
    let lan = download_with_set(&database, directory.path(), pasted).await;

    let strict = database.download_sources(strict).await.expect("sources");
    assert!(!strict.is_empty());
    assert!(strict.iter().all(|source| !source.local_network));
    let lan = database.download_sources(lan).await.expect("sources");
    assert!(!lan.is_empty());
    assert!(lan.iter().all(|source| source.local_network));
}

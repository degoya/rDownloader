//! What a continued FTP transfer may build on (RD-1190-01): a timestamp that can no longer be
//! read (TR-03), and a mirror that acknowledges `REST` and sends from the first byte (TR-02).

use std::{collections::HashMap, sync::Arc};

use rd_core::{ChunkId, RemoteTarget};
use rd_ftp::FtpRunner;
use rd_http::{
    CheckpointSink, ChunkSpec, DownloadEngine, HttpDownloadError, MultiSourceRequest,
    RangeTransport, SourceEndpoint, SourceLedger,
};
use rd_scheduler::{ExternalRunner, RunOutcome};
use tokio_util::sync::CancellationToken;

use crate::{
    fixture_server::{Behaviour, RemoteFile},
    harness::{Harness, payload, run_limits},
};

/// The first attempt recorded the file's timestamp; the second cannot read one. Size alone
/// used to continue the file — the same size is exactly what a replaced file of the same
/// length has.
#[tokio::test]
async fn a_recorded_timestamp_the_server_no_longer_reports_refuses_the_resume() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let _ = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("first run");
    assert!(
        harness.part_path(&file).exists(),
        "the first run kept nothing"
    );

    harness.fixture.set_behaviour(Behaviour {
        refuse_mdtm: true,
        ..Behaviour::default()
    });
    let reloaded = harness
        .database
        .get_download(file.id)
        .await
        .expect("reload")
        .expect("row");
    let outcome = harness
        .runner()
        .run(
            &reloaded,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("second run");

    match outcome {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_ftp::FILE_CHANGED));
        }
        other => panic!("expected a refused resume, got {other:?}"),
    }
    assert!(harness.part_path(&file).exists());
    assert!(!harness.destination.join("movie.bin").exists());
}

/// Records nothing; the case reads the part file and the outcome.
struct Ledger;

#[async_trait::async_trait]
impl CheckpointSink for Ledger {
    async fn commit(&self, _chunk_id: ChunkId, _committed_offset: u64) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl SourceLedger for Ledger {
    async fn source_delivered(&self, _position: u32, _bytes: u64) -> anyhow::Result<()> {
        Ok(())
    }

    async fn source_failed(
        &self,
        _position: u32,
        _code: &str,
        _retry_after_seconds: Option<u64>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn source_isolated(&self, _position: u32, _code: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn chunk_marked(
        &self,
        _chunk_id: ChunkId,
        _position: Option<u32>,
        _verified: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn chunk_rewound(&self, _chunk_id: ChunkId, _committed: u64) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A server that answers `REST` with `350` and sends from byte 0 anyway hands a chunk read
/// only to its end exactly as many bytes as it asked for: the head of the file, which would
/// land at the chunk's offset with no length to give it away. Without a hash, the mirror is
/// never asked for that chunk, and the file's head is never written inside it.
#[tokio::test]
async fn a_mirror_that_ignores_rest_is_not_asked_inside_the_file_without_a_hash() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/mirror.bin", RemoteFile::new(payload()));
    harness.credential().await;
    harness.fixture.set_behaviour(Behaviour {
        ignore_rest: true,
        ..Behaviour::default()
    });
    let url = format!("ftp://127.0.0.1:{}/pub/mirror.bin", harness.fixture.port)
        .parse()
        .expect("url");
    let source = SourceEndpoint {
        position: 0,
        via: Some(RangeTransport {
            source: FtpRunner::new(harness.service.clone())
                .range_source()
                .expect("FTP serves mirrors"),
            target: RemoteTarget::parse(&url).expect("target"),
            policy: None,
        }),
        url,
        headers: Vec::new(),
    };
    let total = payload().len() as u64;
    let half = total / 2;
    let chunks = vec![
        ChunkSpec {
            id: ChunkId::new(),
            start: 0,
            end: Some(half),
            committed: 0,
        },
        ChunkSpec {
            id: ChunkId::new(),
            start: half,
            end: Some(total),
            committed: half,
        },
    ];
    let part_path = harness.destination.join("mirror.part");

    let outcome = DownloadEngine::new(Default::default(), rd_limits::ScopedLimiter::unlimited())
        .download_from_sources(
            MultiSourceRequest {
                part_path: part_path.clone(),
                total_bytes: total,
                chunks,
                sources: vec![source],
                parallel_sources: 1,
                pieces: None,
                unverified: HashMap::new(),
                whole_file_hash: false,
            },
            Arc::new(Ledger),
            CancellationToken::new(),
        )
        .await;

    match outcome {
        Err(HttpDownloadError::Failure(failure)) => {
            assert_eq!(failure.code.as_deref(), Some("mirror.offset_unverified"));
        }
        other => panic!("expected the inner chunk to wait for a provable source, got {other:?}"),
    }
    let written = tokio::fs::read(&part_path).await.expect("part file");
    let half = usize::try_from(half).expect("half");
    assert_eq!(written[..half], payload()[..half]);
    assert!(
        written[half..].iter().all(|byte| *byte == 0),
        "the file's head was written at the chunk's offset"
    );
}

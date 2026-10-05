//! A package's own speed limit (RD-1100-01), measured against a local server: two packages load
//! the same payload at once, the limited one at its limit, the other at full speed.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{Router, routing::get};
use rd_core::{ChunkId, DownloadKind, PackageId};
use rd_http::{CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, DownloadRequest};
use rd_limits::{LimiterRegistry, TransferScope};
use tokio_util::sync::CancellationToken;

const RATE: u64 = 200_000;
const TOTAL: usize = 600_000;

struct NoopCheckpoint;

#[async_trait::async_trait]
impl CheckpointSink for NoopCheckpoint {
    async fn commit(&self, _chunk_id: ChunkId, _committed_offset: u64) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn origin() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    let router = Router::new().route("/payload", get(|| async { vec![7_u8; TOTAL] }));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    address
}

/// Downloads the payload under `scope` and answers how long it took.
async fn timed_download(
    registry: &LimiterRegistry,
    scope: TransferScope,
    address: SocketAddr,
    part_path: std::path::PathBuf,
) -> Duration {
    let engine = DownloadEngine::new(reqwest::Client::new(), registry.scoped(scope));
    let started = Instant::now();
    let outcome = engine
        .download(
            DownloadRequest::get(
                format!("http://{address}/payload").parse().expect("url"),
                part_path.clone(),
                Some(TOTAL as u64),
                vec![ChunkSpec {
                    id: ChunkId::new(),
                    start: 0,
                    end: None,
                    committed: 0,
                }],
            ),
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await
        .expect("download");
    let elapsed = started.elapsed();
    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(
        tokio::fs::metadata(&part_path).await.expect("part").len(),
        TOTAL as u64
    );
    elapsed
}

fn file_of(package: PackageId) -> TransferScope {
    TransferScope::for_download(DownloadKind::Http, Some("127.0.0.1"), None, None)
        .in_package(package)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_limited_package_loads_at_its_limit_while_another_runs_at_full_speed() {
    let address = origin().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let registry = LimiterRegistry::new();
    let (limited, free) = (PackageId::new(), PackageId::new());
    registry.set_package_limits(&[(limited, RATE)]);

    let (slow, fast) = tokio::join!(
        timed_download(
            &registry,
            file_of(limited),
            address,
            directory.path().join("limited.part"),
        ),
        timed_download(
            &registry,
            file_of(free),
            address,
            directory.path().join("free.part"),
        ),
    );

    // The bucket holds one second's worth, so everything past the first `RATE` bytes is paced:
    // 400 000 bytes at 200 000 per second is 2 s at the least.
    let paced = (TOTAL as u64 - RATE) as f64 / RATE as f64;
    let slow = slow.as_secs_f64();
    assert!(slow >= paced * 0.95, "the limited package took {slow:.2} s");
    let rate = (TOTAL as u64 - RATE) as f64 / slow;
    assert!(rate <= RATE as f64 * 1.05, "measured {rate:.0} B/s");
    assert!(
        fast.as_secs_f64() < paced / 2.0,
        "the other package took {:.2} s",
        fast.as_secs_f64()
    );
}

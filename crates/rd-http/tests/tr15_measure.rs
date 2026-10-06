//! TR-15 (audit 1.9.1, RD-191-03; RD-1120-18): what the engine's write path costs per frame,
//! measured rather than guessed.
//!
//! `Worker::write_bytes` (`src/engine/write.rs`) copies every frame the response stream hands
//! it (`bytes.to_vec()`, with a transform or without) and `PartFile::write_at` sends each one to
//! the blocking pool on its own. This harness downloads one payload from a local origin several
//! ways and prints the throughput of each:
//!
//! - `network only`: the frames are read and dropped, the ceiling of the connection;
//! - `copy only`: each frame is copied as the engine does and dropped, the copy's own cost;
//! - `copy + write/frame`: the engine's inner path, a copy and a `write_at` per frame;
//! - `write/frame, no copy`: the frame itself goes to the blocking pool, one task per frame;
//! - `copy into 1 MiB batches`: one copy into a batch, one `write_at` per MiB;
//! - `engine`: `DownloadEngine` as shipped, its checkpoints (8 MiB or 2 s, each a sync) included.
//!
//! The writing variants sync once at the end. Nothing is asserted about speed - a machine's
//! numbers are its own - only that every byte arrived. Ignored, so no ordinary run pays for it;
//! run it in release mode on an otherwise idle machine:
//!
//!     cargo nextest run --release -p rd-http --test tr15_measure --run-ignored only --no-capture
//!
//! `RD_TR15_MIB` sets the payload (default 512 MiB), `RD_TR15_DIR` the folder written into
//! (default the system's temporary folder; point it at the disk downloads go to). The numbers
//! and the decision belong in the job file of RD-1120-18.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{Router, body::Body, routing::get};
use bytes::Bytes;
use futures_util::StreamExt;
use rd_core::ChunkId;
use rd_files::PartFile;
use rd_http::{CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, DownloadRequest};
use tokio_util::sync::CancellationToken;

const MIB: usize = 1024 * 1024;
/// Each variant runs this often; the best and the median are printed.
const ROUNDS: usize = 3;
/// What the origin hands its connection at once.
const SERVED_CHUNK: usize = 64 * 1024;
const BATCH: usize = MIB;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Variant {
    NetworkOnly,
    CopyOnly,
    CopyAndWritePerFrame,
    WritePerFrameNoCopy,
    Batched,
    Engine,
}

impl Variant {
    const ALL: [Self; 6] = [
        Self::NetworkOnly,
        Self::CopyOnly,
        Self::CopyAndWritePerFrame,
        Self::WritePerFrameNoCopy,
        Self::Batched,
        Self::Engine,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::NetworkOnly => "network only",
            Self::CopyOnly => "copy only",
            Self::CopyAndWritePerFrame => "copy + write/frame",
            Self::WritePerFrameNoCopy => "write/frame, no copy",
            Self::Batched => "copy into 1 MiB batches",
            Self::Engine => "engine",
        }
    }
}

struct Measured {
    bytes: u64,
    frames: u64,
    elapsed: Duration,
}

impl Measured {
    fn mib_per_second(&self) -> f64 {
        self.bytes as f64 / MIB as f64 / self.elapsed.as_secs_f64()
    }
}

struct NoopCheckpoint;

#[async_trait::async_trait]
impl CheckpointSink for NoopCheckpoint {
    async fn commit(&self, _chunk_id: ChunkId, _committed_offset: u64) -> anyhow::Result<()> {
        Ok(())
    }
}

fn payload_bytes() -> usize {
    std::env::var("RD_TR15_MIB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(512)
        * MIB
}

fn scratch_root() -> PathBuf {
    std::env::var_os("RD_TR15_DIR").map_or_else(std::env::temp_dir, PathBuf::from)
}

/// Serves `total` bytes at `/payload` from one shared buffer, with a length, as a host does.
async fn origin(total: usize) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    let chunk = Bytes::from(vec![7_u8; SERVED_CHUNK]);
    let router = Router::new().route(
        "/payload",
        get(move || {
            let chunk = chunk.clone();
            async move {
                let rest = total % SERVED_CHUNK;
                let frames = std::iter::repeat_n(chunk.clone(), total / SERVED_CHUNK)
                    .chain((rest > 0).then(|| chunk.slice(..rest)))
                    .map(Ok::<_, std::io::Error>);
                (
                    [(axum::http::header::CONTENT_LENGTH, total.to_string())],
                    Body::from_stream(futures_util::stream::iter(frames)),
                )
            }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    address
}

#[cfg(unix)]
fn write_all_at(file: &std::fs::File, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, bytes, offset)
}

#[cfg(windows)]
fn write_all_at(file: &std::fs::File, mut offset: u64, mut bytes: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !bytes.is_empty() {
        let written = file.seek_write(bytes, offset)?;
        if written == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[written..];
        offset += written as u64;
    }
    Ok(())
}

async fn engine(url: &str, path: &Path, total: u64) -> Measured {
    let engine = DownloadEngine::new(
        reqwest::Client::new(),
        rd_limits::ScopedLimiter::unlimited(),
    );
    let started = Instant::now();
    let outcome = engine
        .download(
            DownloadRequest::get(
                url.parse().expect("url"),
                path.to_path_buf(),
                Some(total),
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
    assert_eq!(outcome, DownloadOutcome::Complete);
    Measured {
        bytes: total,
        frames: 0,
        elapsed: started.elapsed(),
    }
}

/// One download of the payload into `path` the way `variant` says.
async fn run(variant: Variant, client: &reqwest::Client, url: &str, path: &Path) -> Measured {
    let part = PartFile::open(path.to_path_buf(), None)
        .await
        .expect("part file");
    let raw = Arc::new(
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("a second handle"),
    );
    let started = Instant::now();
    let mut body = client
        .get(url)
        .send()
        .await
        .expect("response")
        .bytes_stream();
    let (mut position, mut frames) = (0_u64, 0_u64);
    let (mut batch, mut batch_start) = (Vec::with_capacity(BATCH), 0_u64);
    while let Some(frame) = body.next().await {
        let frame = frame.expect("frame");
        let length = frame.len() as u64;
        frames += 1;
        match variant {
            Variant::NetworkOnly | Variant::Engine => {}
            Variant::CopyOnly => {
                std::hint::black_box(frame.to_vec());
            }
            Variant::CopyAndWritePerFrame => {
                part.write_at(position, frame.to_vec())
                    .await
                    .expect("write");
            }
            Variant::WritePerFrameNoCopy => {
                let raw = Arc::clone(&raw);
                tokio::task::spawn_blocking(move || write_all_at(&raw, position, &frame))
                    .await
                    .expect("join")
                    .expect("write");
            }
            Variant::Batched => {
                batch.extend_from_slice(&frame);
                if batch.len() >= BATCH {
                    let full = std::mem::replace(&mut batch, Vec::with_capacity(BATCH));
                    part.write_at(batch_start, full).await.expect("write");
                    batch_start = position + length;
                }
            }
        }
        position += length;
    }
    if !batch.is_empty() {
        part.write_at(batch_start, batch).await.expect("write");
    }
    part.sync_data().await.expect("sync");
    Measured {
        bytes: position,
        frames,
        elapsed: started.elapsed(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement, run by hand: see the module comment"]
async fn tr15_write_path_per_frame() {
    let total = payload_bytes();
    let address = origin(total).await;
    let url = format!("http://{address}/payload");
    let client = reqwest::Client::new();
    let scratch = tempfile::tempdir_in(scratch_root()).expect("scratch folder");
    println!(
        "TR-15: {} MiB from {address} into {}, best and median of {ROUNDS}",
        total / MIB,
        scratch.path().display()
    );
    for variant in Variant::ALL {
        let mut rounds = Vec::with_capacity(ROUNDS);
        for round in 0..ROUNDS {
            let path = scratch.path().join(format!("{variant:?}-{round}.part"));
            let measured = match variant {
                Variant::Engine => engine(&url, &path, total as u64).await,
                _ => run(variant, &client, &url, &path).await,
            };
            assert_eq!(
                measured.bytes,
                total as u64,
                "{} lost bytes",
                variant.label()
            );
            if variant != Variant::NetworkOnly && variant != Variant::CopyOnly {
                assert_eq!(
                    tokio::fs::metadata(&path).await.expect("written").len(),
                    total as u64
                );
            }
            let _ = tokio::fs::remove_file(&path).await;
            rounds.push(measured);
        }
        rounds.sort_by_key(|measured| measured.elapsed);
        let (best, median) = (&rounds[0], &rounds[ROUNDS / 2]);
        let frames = best.bytes.checked_div(best.frames).map_or_else(
            || "      -".to_owned(),
            |per_frame| format!("{per_frame:7}"),
        );
        println!(
            "  {:<26} best {:8.1} MiB/s  median {:8.1} MiB/s  bytes/frame {frames}  frames {}",
            variant.label(),
            best.mib_per_second(),
            median.mib_per_second(),
            best.frames,
        );
    }
}

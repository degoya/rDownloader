//! The transform on the engine's own write path (RD-110-33).
//!
//! `transform_vectors.rs` proves the arithmetic against fixed vectors. This file proves the
//! part that only the engine can be wrong about: that the bytes reach the part file already
//! transformed, that a chunk MAC survives a delivery boundary, that a wrong integrity value
//! ends the attempt with the partial file still on disk, and that a continuation resumes only
//! what the same description wrote.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{Router, extract::State, http::HeaderMap, response::Response, routing::get};
use rd_core::{
    CIPHER_AES_128_CTR, CODE_INTEGRITY_MISMATCH, ChunkId, CipherSpec, ContentTransform,
    INTEGRITY_CBC_MAC_CHAIN, IntegritySpec, TransformKey,
};
use rd_http::{
    CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
    StreamTransform, TransformCheckpoint, TransformPlan,
};
use rd_limits::ScopedLimiter;
use tokio_util::sync::CancellationToken;

const SIZE: usize = 512 * 1024;
const KEY: &str = "0c4c44e128eaee7a40bcbd4ffec19617";
const NONCE: &str = "801b72fd9641ccfa";
/// The chunk boundaries the 512 KiB fixture is condensed over, four of 128 KiB.
const BOUNDARIES: [u64; 4] = [131_072, 262_144, 393_216, 524_288];
/// What those four chunks condense and fold to.
const EXPECTED: &str = "42e920fd56fd5e0b";

fn hex(value: &str) -> Vec<u8> {
    hex::decode(value).expect("a hex literal in this file")
}

fn plaintext() -> Vec<u8> {
    (0..SIZE).map(|index| (index % 251) as u8).collect()
}

fn description(expected: &str, boundaries: Vec<u64>, nonce: &str) -> ContentTransform {
    ContentTransform {
        cipher: CipherSpec {
            algorithm: CIPHER_AES_128_CTR.to_owned(),
            key_reference: Some(format!("vault://{nonce}")),
            nonce: hex(nonce),
            first_block: 0,
        },
        integrity: Some(IntegritySpec {
            algorithm: INTEGRITY_CBC_MAC_CHAIN.to_owned(),
            boundaries,
            iv: [hex(NONCE), hex(NONCE)].concat(),
            expected: hex(expected),
        }),
    }
}

fn transform(description: ContentTransform) -> Arc<StreamTransform> {
    Arc::new(
        StreamTransform::new(description, &TransformKey::new(hex(KEY)))
            .expect("the fixture is computable"),
    )
}

/// The ciphertext the origin serves: the fixture plaintext, encrypted by the same routine
/// that will decrypt it. The vectors file is what proves that routine right; here it only
/// has to produce something the engine must turn back into `plaintext()`.
fn ciphertext() -> Vec<u8> {
    let transform = transform(description(EXPECTED, BOUNDARIES.to_vec(), NONCE));
    let mut bytes = plaintext();
    transform.apply(0, &mut bytes);
    bytes
}

/// What the database would have been told, and what a continuation reads back.
#[derive(Default)]
struct Recorder {
    committed: Mutex<BTreeMap<ChunkId, u64>>,
    macs: Mutex<Vec<(usize, [u8; 16])>>,
}

#[async_trait::async_trait]
impl CheckpointSink for Recorder {
    async fn commit(&self, chunk_id: ChunkId, committed_offset: u64) -> anyhow::Result<()> {
        self.committed
            .lock()
            .expect("lock")
            .insert(chunk_id, committed_offset);
        Ok(())
    }

    async fn commit_chunk_mac(&self, index: u64, mac: [u8; 16]) -> anyhow::Result<()> {
        self.macs.lock().expect("lock").push((index as usize, mac));
        Ok(())
    }
}

/// What the origin was asked for, so a case can prove a fallback really used one connection.
#[derive(Default)]
struct Origin {
    ranges: Mutex<Vec<Option<String>>>,
}

async fn serve() -> (SocketAddr, Arc<Origin>) {
    let origin = Arc::new(Origin::default());
    let router = Router::new()
        .route("/payload", get(payload))
        .with_state(Arc::clone(&origin));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (address, origin)
}

async fn payload(State(origin): State<Arc<Origin>>, headers: HeaderMap) -> Response {
    let range = headers
        .get("range")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    origin.ranges.lock().expect("lock").push(range.clone());
    let body = ciphertext();
    let (start, end) = match range.as_deref().and_then(parse_range) {
        Some(bounds) => bounds,
        None => (0, body.len()),
    };
    let slice = body[start..end].to_vec();
    Response::builder()
        .status(if range.is_some() { 206 } else { 200 })
        .header(
            "content-range",
            format!("bytes {start}-{}/{}", end - 1, body.len()),
        )
        .body(axum::body::Body::from(slice))
        .expect("response")
}

fn parse_range(value: &str) -> Option<(usize, usize)> {
    let rest = value.strip_prefix("bytes=")?;
    let (from, to) = rest.split_once('-')?;
    let start: usize = from.parse().ok()?;
    let end = if to.is_empty() {
        SIZE
    } else {
        to.parse::<usize>().ok()? + 1
    };
    Some((start, end.min(SIZE)))
}

fn chunks(count: usize) -> Vec<ChunkSpec> {
    let size = (SIZE as u64).div_ceil(count as u64);
    (0..count)
        .map(|index| ChunkSpec {
            id: ChunkId::new(),
            start: index as u64 * size,
            end: Some(((index as u64 + 1) * size).min(SIZE as u64)),
            committed: index as u64 * size,
        })
        .collect()
}

async fn run(
    address: SocketAddr,
    part: &std::path::Path,
    chunks: Vec<ChunkSpec>,
    plan: Option<TransformPlan>,
    recorder: Arc<Recorder>,
) -> Result<DownloadOutcome, HttpDownloadError> {
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());
    engine
        .download(
            DownloadRequest {
                use_ranges: true,
                transform: plan,
                ..DownloadRequest::get(
                    format!("http://{address}/payload").parse().expect("url"),
                    part.to_path_buf(),
                    Some(SIZE as u64),
                    chunks,
                )
            },
            recorder,
            CancellationToken::new(),
        )
        .await
}

/// Four connections whose boundaries are the provider's, and the file that comes out.
#[tokio::test]
async fn parallel_chunks_on_the_provider_boundaries_produce_the_expected_file() {
    let (address, origin) = serve().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part = directory.path().join("payload.part");
    let recorder = Arc::new(Recorder::default());
    let outcome = run(
        address,
        &part,
        chunks(4),
        Some(TransformPlan {
            transform: transform(description(EXPECTED, BOUNDARIES.to_vec(), NONCE)),
            checkpoint: TransformCheckpoint::default(),
        }),
        Arc::clone(&recorder),
    )
    .await
    .expect("the integrity value matched");
    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(std::fs::read(&part).expect("part file"), plaintext());
    // Four connections, and four chunk MACs recorded.
    assert_eq!(origin.ranges.lock().expect("lock").len(), 4);
    let mut recorded = recorder.macs.lock().expect("lock").clone();
    recorded.sort_by_key(|(index, _)| *index);
    assert_eq!(
        recorded.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}

/// Boundaries that do not line up give up the parallelism rather than computing a MAC over
/// part of a chunk. Three connections over a file chunked into four is exactly that case.
#[tokio::test]
async fn a_layout_that_does_not_line_up_falls_back_to_one_stream() {
    let (address, origin) = serve().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part = directory.path().join("payload.part");
    let recorder = Arc::new(Recorder::default());
    run(
        address,
        &part,
        chunks(3),
        Some(TransformPlan {
            transform: transform(description(EXPECTED, BOUNDARIES.to_vec(), NONCE)),
            checkpoint: TransformCheckpoint::default(),
        }),
        Arc::clone(&recorder),
    )
    .await
    .expect("the integrity value matched");
    assert_eq!(std::fs::read(&part).expect("part file"), plaintext());
    let ranges = origin.ranges.lock().expect("lock").clone();
    assert_eq!(ranges.len(), 1, "the run kept more than one connection");
    assert_eq!(ranges[0].as_deref(), Some("bytes=0-524287"));
}

/// The one thing that tells a wrong key from a correct download.
///
/// The attempt fails, and the partial file stays: a caller promotes a part file only after
/// the engine says `Complete`, so a mismatch can never be presented as a finished download.
#[tokio::test]
async fn a_wrong_integrity_value_fails_the_attempt_and_keeps_the_partial_file() {
    let (address, _origin) = serve().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part = directory.path().join("payload.part");
    let recorder = Arc::new(Recorder::default());
    let error = run(
        address,
        &part,
        chunks(4),
        Some(TransformPlan {
            transform: transform(description("0000000000000000", BOUNDARIES.to_vec(), NONCE)),
            checkpoint: TransformCheckpoint::default(),
        }),
        Arc::clone(&recorder),
    )
    .await
    .expect_err("the integrity value did not match");
    match error {
        HttpDownloadError::Failure(failure) => {
            assert_eq!(failure.code.as_deref(), Some(CODE_INTEGRITY_MISMATCH));
        }
        other => panic!("unexpected error: {other}"),
    }
    assert!(part.exists(), "the partial file was removed");
    assert_eq!(
        std::fs::metadata(&part).expect("metadata").len(),
        SIZE as u64
    );
}

/// A continuation with the same description keeps what was recorded and fetches only the rest.
#[tokio::test]
async fn the_same_description_continues_where_it_stopped() {
    let (address, origin) = serve().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part = directory.path().join("payload.part");

    // First pass: one connection over the first half, so two chunk MACs are recorded.
    let first = Arc::new(Recorder::default());
    let described = description(EXPECTED, BOUNDARIES.to_vec(), NONCE);
    let half = vec![ChunkSpec {
        id: ChunkId::new(),
        start: 0,
        end: Some(262_144),
        committed: 0,
    }];
    run(
        address,
        &part,
        half,
        Some(TransformPlan {
            transform: transform(described.clone()),
            checkpoint: TransformCheckpoint::default(),
        }),
        Arc::clone(&first),
    )
    .await
    .expect("the first half");
    let recorded = first.macs.lock().expect("lock").clone();
    assert_eq!(recorded.len(), 2, "two chunks were finished");

    // Second pass: the same description, the rest of the file, and the recorded MACs.
    let second = Arc::new(Recorder::default());
    let built = transform(described);
    let outcome = run(
        address,
        &part,
        vec![ChunkSpec {
            id: ChunkId::new(),
            start: 0,
            end: Some(SIZE as u64),
            committed: 262_144,
        }],
        Some(TransformPlan {
            transform: Arc::clone(&built),
            checkpoint: TransformCheckpoint {
                fingerprint: Some(built.fingerprint().to_owned()),
                macs: recorded,
            },
        }),
        Arc::clone(&second),
    )
    .await
    .expect("the integrity value matched");
    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(std::fs::read(&part).expect("part file"), plaintext());
    // The continuation asked for the tail only, and closed the two remaining chunks.
    let ranges = origin.ranges.lock().expect("lock").clone();
    assert_eq!(
        ranges.last().cloned().flatten().as_deref(),
        Some("bytes=262144-524287")
    );
    assert_eq!(second.macs.lock().expect("lock").len(), 2);
}

/// A continuation with a different description starts over rather than resuming somebody
/// else's state.
#[tokio::test]
async fn a_different_description_starts_over() {
    let (address, origin) = serve().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part = directory.path().join("payload.part");
    let recorder = Arc::new(Recorder::default());
    let built = transform(description(EXPECTED, BOUNDARIES.to_vec(), NONCE));
    let outcome = run(
        address,
        &part,
        vec![ChunkSpec {
            id: ChunkId::new(),
            start: 0,
            end: Some(SIZE as u64),
            committed: 262_144,
        }],
        Some(TransformPlan {
            transform: Arc::clone(&built),
            checkpoint: TransformCheckpoint {
                // A fingerprint from another stream: nothing here describes this file.
                fingerprint: Some("0123456789abcdef0123456789abcdef".to_owned()),
                macs: vec![(0, [1_u8; 16]), (1, [2_u8; 16])],
            },
        }),
        Arc::clone(&recorder),
    )
    .await
    .expect("the integrity value matched");
    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(std::fs::read(&part).expect("part file"), plaintext());
    // Started over: the whole file was fetched, and all four chunk MACs were computed again.
    let ranges = origin.ranges.lock().expect("lock").clone();
    assert_eq!(ranges[0].as_deref(), Some("bytes=0-524287"));
    assert_eq!(recorder.macs.lock().expect("lock").len(), 4);
}

/// A layout that is not responsible for the whole file gets its bytes transformed and no
/// verdict: the integrity value covers the file, and half of one cannot be checked.
#[tokio::test]
async fn a_layout_that_covers_only_part_of_the_file_is_not_judged() {
    let (address, _origin) = serve().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part = directory.path().join("payload.part");
    let recorder = Arc::new(Recorder::default());
    let outcome = run(
        address,
        &part,
        vec![ChunkSpec {
            id: ChunkId::new(),
            start: 0,
            end: Some(262_144),
            committed: 0,
        }],
        Some(TransformPlan {
            transform: transform(description(EXPECTED, BOUNDARIES.to_vec(), NONCE)),
            checkpoint: TransformCheckpoint::default(),
        }),
        Arc::clone(&recorder),
    )
    .await
    .expect("half a file is not a mismatch");
    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(recorder.macs.lock().expect("lock").len(), 2);
    assert_eq!(
        std::fs::read(&part).expect("part file")[..262_144],
        plaintext()[..262_144]
    );
}

/// An ordinary download -- no description at all -- runs exactly the code it ran before.
#[tokio::test]
async fn a_download_without_a_description_is_untouched() {
    let (address, _origin) = serve().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part = directory.path().join("payload.part");
    let recorder = Arc::new(Recorder::default());
    run(address, &part, chunks(4), None, Arc::clone(&recorder))
        .await
        .expect("an ordinary download");
    assert_eq!(std::fs::read(&part).expect("part file"), ciphertext());
    assert!(recorder.macs.lock().expect("lock").is_empty());
}

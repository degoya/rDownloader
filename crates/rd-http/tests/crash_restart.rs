//! Crash and restart, for the HTTP engine — Axis A of the RD-140-04 recovery matrix.
//!
//! Every other test of this engine runs a transfer to the end. That is the one shape of run
//! that cannot lose data, and it is not how downloads actually stop: they stop because the
//! process was killed, the machine lost power, or the container was replaced mid-write. The
//! interesting instant is between putting bytes on disk and recording that they are there,
//! and it lasts microseconds, so it is not reachable by timing — it has to be addressable.
//! `rd_core::failpoint!` makes it so; see `crates/rd-core/src/failpoint.rs` for why this is
//! a hand-rolled thirty lines rather than the `fail` crate.
//!
//! ## The invariants
//!
//! Every case here asserts the same four things, because they are what "safe to resume"
//! means and each one has its own way of going wrong:
//!
//! 1. **No confirmed byte is invented.** The recorded offset never exceeds what was actually
//!    fetched. Getting this wrong produces a file with a hole in it that passes every length
//!    check.
//! 2. **No confirmed byte is overwritten with different content.** A resume rewrites the tail
//!    after the checkpoint and nothing before it.
//! 3. **Resuming reaches the same bytes as an uninterrupted run.** Compared by SHA-256
//!    against a reference download, so a corruption that preserves the length still fails.
//! 4. **Nothing is left behind.** No stray part or staging file survives the interruption.
//!
//! ## Why this is not a `SIGKILL`
//!
//! A real kill proves one extra thing — that the operating system's write really did land —
//! and costs a spawned binary, a port, and a second of wall clock per case. It is worth
//! paying for a handful of end-to-end scenarios, which is Axis B, in CI. It is not worth
//! paying sixty times over for the state machine, which is what this file covers: returning
//! an error at the crash point drops the whole worker, the part file handle included, which
//! is the same state a restart finds.

#![cfg(feature = "failpoints")]

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{Router, extract::State, http::HeaderMap, response::Response, routing::get};
use rd_core::{ChunkId, failpoint::FailpointGuard};
use rd_http::{
    CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
    StreamTransform, TransformPlan,
};
use rd_limits::ScopedLimiter;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// The payload every case transfers.
///
/// Deliberately larger than the engine's 8 MiB checkpoint threshold, and by more than one
/// multiple of it: a payload that fits inside one checkpoint interval can only ever crash
/// before the first commit or after the last, which is the pair of easy cases. Twenty
/// mebibytes puts two interim commits in the middle, so a resume has to land on an offset
/// that is neither zero nor the end.
const PAYLOAD_BYTES: usize = 20 * 1024 * 1024;

/// The payload, built once. Twenty mebibytes is cheap to hold and expensive to rebuild for
/// every request a case makes.
fn payload() -> &'static [u8] {
    static PAYLOAD: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    PAYLOAD.get_or_init(|| {
        // Non-repeating with a period coprime to every power of two, so a resume that lands
        // on the wrong offset produces different bytes rather than accidentally identical ones.
        (0..PAYLOAD_BYTES)
            .map(|index| (index % 251) as u8)
            .collect()
    })
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Where the provider chunks of the transformed fixture end, four over the payload.
const MAC_BOUNDARIES: [u64; 4] = [
    PAYLOAD_BYTES as u64 / 4,
    PAYLOAD_BYTES as u64 / 2,
    PAYLOAD_BYTES as u64 / 4 * 3,
    PAYLOAD_BYTES as u64,
];

/// The transform the `http.after_chunk_mac` case runs under.
///
/// Its expected integrity value is a placeholder: that case is about what a restart recomputes,
/// and a run that reaches the check at all has already recomputed everything it must.
fn transform_fixture() -> Arc<StreamTransform> {
    Arc::new(
        StreamTransform::new(
            rd_core::ContentTransform {
                cipher: rd_core::CipherSpec {
                    algorithm: rd_core::CIPHER_AES_128_CTR.to_owned(),
                    key_reference: Some("vault://crash-case".to_owned()),
                    nonce: vec![1, 2, 3, 4, 5, 6, 7, 8],
                    first_block: 0,
                },
                integrity: Some(rd_core::IntegritySpec {
                    algorithm: rd_core::INTEGRITY_CBC_MAC_CHAIN.to_owned(),
                    boundaries: MAC_BOUNDARIES.to_vec(),
                    iv: [1, 2, 3, 4, 5, 6, 7, 8].repeat(2),
                    expected: vec![0; 8],
                }),
            },
            &rd_core::TransformKey::new(vec![9_u8; 16]),
        )
        .expect("the fixture describes primitives this build implements"),
    )
}

/// A checkpoint sink that records what the database would have been told.
#[derive(Default)]
struct RecordingCheckpoint {
    committed: Mutex<std::collections::HashMap<ChunkId, u64>>,
    chunk_macs: Mutex<Vec<(usize, [u8; 16])>>,
}

impl RecordingCheckpoint {
    /// The provider-chunk MACs recorded so far, in the shape a checkpoint carries them.
    fn chunk_macs(&self) -> Vec<(usize, [u8; 16])> {
        self.chunk_macs.lock().expect("lock").clone()
    }

    /// The highest offset this chunk was ever recorded at — what a restart would resume from.
    fn committed(&self, chunk: ChunkId) -> u64 {
        self.committed
            .lock()
            .expect("lock")
            .get(&chunk)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl CheckpointSink for RecordingCheckpoint {
    async fn commit(&self, chunk_id: ChunkId, committed_offset: u64) -> anyhow::Result<()> {
        let mut committed = self.committed.lock().expect("lock");
        let entry = committed.entry(chunk_id).or_insert(0);
        // The engine must never move a checkpoint backwards; if it did, the assertion below
        // would hide it behind a max().
        assert!(
            committed_offset >= *entry,
            "checkpoint moved backwards: {} -> {committed_offset}",
            *entry
        );
        *entry = committed_offset;
        Ok(())
    }

    async fn commit_chunk_mac(&self, index: u64, mac: [u8; 16]) -> anyhow::Result<()> {
        self.chunk_macs
            .lock()
            .expect("lock")
            .push((index as usize, mac));
        Ok(())
    }
}

/// What the origin was asked for, so a case can prove a resume did not re-fetch from zero.
#[derive(Default)]
struct Origin {
    requests: AtomicUsize,
    ranges: Mutex<Vec<Option<String>>>,
}

async fn serve_payload(origin: Arc<Origin>) -> SocketAddr {
    async fn handler(State(origin): State<Arc<Origin>>, headers: HeaderMap) -> Response {
        origin.requests.fetch_add(1, Ordering::SeqCst);
        let range = headers
            .get(axum::http::header::RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        origin.ranges.lock().expect("lock").push(range.clone());

        let body = payload();
        let total = body.len() as u64;
        let start = range
            .as_deref()
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.split('-').next())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let slice = body[start as usize..].to_vec();
        let status = if range.is_some() {
            axum::http::StatusCode::PARTIAL_CONTENT
        } else {
            axum::http::StatusCode::OK
        };
        let mut response = Response::builder()
            .status(status)
            .header(axum::http::header::ACCEPT_RANGES, "bytes")
            .header(axum::http::header::CONTENT_LENGTH, slice.len().to_string());
        if range.is_some() {
            response = response.header(
                axum::http::header::CONTENT_RANGE,
                format!("bytes {start}-{}/{total}", total - 1),
            );
        }
        response
            .body(axum::body::Body::from(slice))
            .expect("response")
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    let app = Router::new()
        .route("/payload", get(handler))
        .with_state(origin);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

/// Runs one transfer attempt, resuming from `committed`.
async fn attempt(
    address: SocketAddr,
    part_path: &std::path::Path,
    chunk: ChunkId,
    committed: u64,
    checkpoints: Arc<RecordingCheckpoint>,
) -> Result<DownloadOutcome, HttpDownloadError> {
    attempt_with_transform(address, part_path, chunk, committed, checkpoints, None).await
}

/// The same, for a stream the host has to transform on the way in (RD-110-33).
async fn attempt_with_transform(
    address: SocketAddr,
    part_path: &std::path::Path,
    chunk: ChunkId,
    committed: u64,
    checkpoints: Arc<RecordingCheckpoint>,
    transform: Option<TransformPlan>,
) -> Result<DownloadOutcome, HttpDownloadError> {
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());
    engine
        .download(
            DownloadRequest {
                use_ranges: true,
                transform,
                ..DownloadRequest::get(
                    format!("http://{address}/payload").parse().expect("url"),
                    part_path.to_path_buf(),
                    Some(PAYLOAD_BYTES as u64),
                    vec![ChunkSpec {
                        id: chunk,
                        start: 0,
                        end: Some(PAYLOAD_BYTES as u64),
                        committed,
                    }],
                )
            },
            checkpoints,
            CancellationToken::new(),
        )
        .await
}

/// The four invariants, applied after a crash and the restart that followed it.
fn assert_invariants(
    label: &str,
    part_path: &std::path::Path,
    recorded_after_crash: u64,
    directory: &std::path::Path,
) {
    let final_bytes = std::fs::read(part_path).expect("read the finished part file");

    // 3. The resumed result is byte-for-byte what an uninterrupted run produces.
    assert_eq!(
        sha256(&final_bytes),
        sha256(payload()),
        "{label}: the resumed file differs from an uninterrupted download"
    );

    // 1. The offset the restart trusted was never beyond the payload.
    assert!(
        recorded_after_crash <= PAYLOAD_BYTES as u64,
        "{label}: recorded {recorded_after_crash} confirmed bytes of a {PAYLOAD_BYTES}-byte file"
    );

    // 2. Everything before the checkpoint is still the original content. Checked explicitly
    //    rather than implied by the hash, so a failure says *which* half went wrong.
    let prefix = recorded_after_crash as usize;
    assert_eq!(
        &final_bytes[..prefix],
        &payload()[..prefix],
        "{label}: bytes before the checkpoint were rewritten with different content"
    );

    // 4. Nothing was left behind.
    let strays: Vec<_> = std::fs::read_dir(directory)
        .expect("read the working directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "payload.part")
        .collect();
    assert!(strays.is_empty(), "{label}: files left behind: {strays:?}");
}

/// Bytes on disk that the database never heard about must not be counted as confirmed.
///
/// The dangerous reading of this state is "the part file is N bytes long, so N bytes are
/// done". The engine must resume from the *recorded* offset instead and re-fetch the tail.
///
/// Reached by composing two crashes, because that is the only deterministic way there. A
/// single crash on the first write leaves an empty checkpoint and an almost-empty file — the
/// easy case. Crashing once after an interim commit and then again on the first write of the
/// resumed attempt puts the file genuinely ahead of the checkpoint, mid-stream, which is the
/// state that actually has to be got right. Pass counts are not used to arrange this: how
/// many buffers a 20 MiB body arrives in is the network's business, not the test's.
#[tokio::test]
async fn a_crash_after_writing_but_before_recording_re_fetches_the_unrecorded_tail() {
    let origin = Arc::new(Origin::default());
    let address = serve_payload(Arc::clone(&origin)).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("payload.part");
    let chunk = ChunkId::new();
    let checkpoints = Arc::new(RecordingCheckpoint::default());

    // First crash: establish a real interim checkpoint to resume from.
    let guard = FailpointGuard::once("http.after_db_checkpoint");
    assert!(
        attempt(address, &part_path, chunk, 0, Arc::clone(&checkpoints))
            .await
            .is_err()
    );
    assert!(guard.fired(), "no interim checkpoint was reached");
    drop(guard);
    let recorded = checkpoints.committed(chunk);
    assert!(
        recorded > 0 && recorded < PAYLOAD_BYTES as u64,
        "expected an interim checkpoint, got {recorded} of {PAYLOAD_BYTES}"
    );

    // Second crash: write past that checkpoint and stop before recording any of it.
    let guard = FailpointGuard::once("http.after_chunk_write");
    assert!(
        attempt(
            address,
            &part_path,
            chunk,
            recorded,
            Arc::clone(&checkpoints)
        )
        .await
        .is_err()
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    // The state under test: the file is ahead of what the database confirmed.
    assert_eq!(
        checkpoints.committed(chunk),
        recorded,
        "a write without a commit advanced the checkpoint"
    );
    let on_disk = std::fs::metadata(&part_path).expect("part file").len();
    assert!(
        on_disk > recorded,
        "the file did not get ahead of the checkpoint ({on_disk} vs {recorded}); \
         the case proves nothing"
    );

    let resumed = attempt(address, &part_path, chunk, recorded, checkpoints)
        .await
        .expect("resume");
    assert_eq!(resumed, DownloadOutcome::Complete);
    assert_invariants("after_chunk_write", &part_path, recorded, directory.path());
}

/// A durable write whose commit never landed falls back to the older checkpoint.
///
/// `sync_data` succeeding does not mean the database recorded anything. Treating the sync as
/// the commit would advance the resume point past bytes nothing confirmed.
#[tokio::test]
async fn a_crash_between_the_sync_and_the_commit_falls_back_to_the_older_checkpoint() {
    let origin = Arc::new(Origin::default());
    let address = serve_payload(Arc::clone(&origin)).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("payload.part");
    let chunk = ChunkId::new();
    let checkpoints = Arc::new(RecordingCheckpoint::default());

    let guard = FailpointGuard::once("http.after_part_sync");
    let crashed = attempt(address, &part_path, chunk, 0, Arc::clone(&checkpoints)).await;
    assert!(crashed.is_err());
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    // The first flush is the one that was interrupted, so nothing was ever recorded.
    let recorded = checkpoints.committed(chunk);
    assert_eq!(
        recorded, 0,
        "a sync without its commit was treated as a checkpoint"
    );

    let resumed = attempt(address, &part_path, chunk, recorded, checkpoints)
        .await
        .expect("resume");
    assert_eq!(resumed, DownloadOutcome::Complete);
    assert_invariants("after_part_sync", &part_path, recorded, directory.path());
}

/// A recorded checkpoint is resumed from exactly, re-fetching nothing before it.
///
/// The opposite failure to the first case: over-cautiously restarting from zero is safe for
/// the data and wrong for the user, who paid for those bytes once already.
#[tokio::test]
async fn a_crash_after_the_commit_resumes_from_exactly_that_offset() {
    let origin = Arc::new(Origin::default());
    let address = serve_payload(Arc::clone(&origin)).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("payload.part");
    let chunk = ChunkId::new();
    let checkpoints = Arc::new(RecordingCheckpoint::default());

    let guard = FailpointGuard::once("http.after_db_checkpoint");
    let crashed = attempt(address, &part_path, chunk, 0, Arc::clone(&checkpoints)).await;
    assert!(crashed.is_err());
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    let recorded = checkpoints.committed(chunk);
    assert!(
        recorded > 0 && recorded < PAYLOAD_BYTES as u64,
        "expected an interim commit, got {recorded} of {PAYLOAD_BYTES}; a completed chunk \
         leaves nothing to resume and the case would prove nothing"
    );

    let resumed = attempt(address, &part_path, chunk, recorded, checkpoints)
        .await
        .expect("resume");
    assert_eq!(resumed, DownloadOutcome::Complete);
    assert_invariants(
        "after_db_checkpoint",
        &part_path,
        recorded,
        directory.path(),
    );

    // The resume asked for the tail, not the whole file again.
    let ranges = origin.ranges.lock().expect("lock").clone();
    let last = ranges.last().expect("a second request").clone();
    assert_eq!(
        last,
        Some(format!("bytes={recorded}-{}", PAYLOAD_BYTES - 1)),
        "the resume re-fetched bytes it had already confirmed"
    );
}

/// `http.after_chunk_mac`: a provider chunk was accounted for and the record of it was lost.
///
/// The one crash point a transformed stream adds, and the byte checkpoints beside it do not
/// cover it: bytes and chunk MACs are written down separately, and the byte checkpoint runs
/// ahead of the MACs. A restart that trusted the bytes alone would resume past a chunk whose
/// MAC nobody holds, and the condensed value at the end would be short one term -- a correct
/// download refused as corrupt. The resume therefore falls back to the last *recorded* MAC,
/// below the byte checkpoint, which is what this case asserts.
#[tokio::test]
async fn a_finished_chunk_mac_that_was_never_recorded_is_computed_again() {
    use rd_http::{TransformCheckpoint, TransformPlan};

    let origin = Arc::new(Origin::default());
    let address = serve_payload(Arc::clone(&origin)).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("payload.part");
    let chunk = ChunkId::new();

    // Stop after the *second* chunk MAC is finished and before it is recorded. The second
    // rather than the first, so the byte checkpoint at 8 MiB has certainly run and the
    // resume has something to rewind from.
    let guard = FailpointGuard::after("http.after_chunk_mac", 1);
    let checkpoints = Arc::new(RecordingCheckpoint::default());
    let crashed = attempt_with_transform(
        address,
        &part_path,
        chunk,
        0,
        Arc::clone(&checkpoints),
        Some(TransformPlan {
            transform: transform_fixture(),
            checkpoint: TransformCheckpoint::default(),
        }),
    )
    .await;
    assert!(crashed.is_err());
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);
    assert_eq!(
        checkpoints.chunk_macs().len(),
        1,
        "the second MAC was recorded although the crash point sits before the write"
    );
    let recorded = checkpoints.committed(chunk);
    assert!(
        recorded > MAC_BOUNDARIES[0],
        "the byte checkpoint did not run past the first provider chunk"
    );

    // The restart: the same description, the recorded bytes, and one recorded MAC.
    let transform = transform_fixture();
    let resumed = attempt_with_transform(
        address,
        &part_path,
        chunk,
        recorded,
        Arc::clone(&checkpoints),
        Some(TransformPlan {
            transform: std::sync::Arc::clone(&transform),
            checkpoint: TransformCheckpoint {
                fingerprint: Some(transform.fingerprint().to_owned()),
                macs: checkpoints.chunk_macs(),
            },
        }),
    )
    .await;
    // The description's expected value is a placeholder, so the run reaches the integrity
    // check and is refused there -- after every chunk MAC was computed, which is the point.
    match resumed {
        Err(HttpDownloadError::Failure(failure)) => assert_eq!(
            failure.code.as_deref(),
            Some(rd_core::CODE_INTEGRITY_MISMATCH),
            "the run stopped before the integrity check"
        ),
        other => panic!("unexpected outcome: {other:?}"),
    }

    // It went back below the byte checkpoint, to the end of the last recorded MAC.
    let ranges = origin.ranges.lock().expect("lock").clone();
    assert_eq!(
        ranges.last().cloned().flatten().as_deref(),
        Some(format!("bytes={}-{}", MAC_BOUNDARIES[0], PAYLOAD_BYTES - 1).as_str()),
        "the resume did not rewind to the last recorded chunk MAC"
    );
    let mut indices: Vec<usize> = checkpoints
        .chunk_macs()
        .into_iter()
        .map(|(index, _)| index)
        .collect();
    indices.sort_unstable();
    indices.dedup();
    assert_eq!(
        indices,
        vec![0, 1, 2, 3],
        "the restart left a provider chunk unaccounted for"
    );

    // Invariant 3, in the shape a transformed stream has it: the file is the transform of the
    // payload, byte for byte. `transform_vectors.rs` is what proves the transform itself.
    let mut expected = payload().to_vec();
    transform.apply(0, &mut expected);
    assert_eq!(
        sha256(&std::fs::read(&part_path).expect("read the part file")),
        sha256(&expected)
    );
}

/// Every registered crash point for this crate has a case here.
///
/// Registering a point and never covering it is the failure this catches: the registry would
/// list an invariant nobody checks, which reads as coverage and is not.
///
/// Both ways of arming count. A point whose interesting instant is the *first* pass is armed
/// with `once`; one whose interesting instant is a later pass -- `http.after_chunk_mac`, which
/// needs a byte checkpoint to already have run -- is armed with `after`, and a check that knew
/// only `once` would refuse a case that exists.
#[test]
fn every_http_crash_point_is_exercised_by_a_case() {
    let source = include_str!("crash_restart.rs");
    for point in rd_core::failpoint::CRASH_POINTS
        .iter()
        .filter(|point| point.owner == "rd-http")
    {
        assert!(
            source.contains(&format!("FailpointGuard::once(\"{}\")", point.name))
                || source.contains(&format!("FailpointGuard::after(\"{}\"", point.name)),
            "{} is registered but no case in this file arms it",
            point.name
        );
    }
}

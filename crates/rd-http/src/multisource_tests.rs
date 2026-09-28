//! Multi-origin fixtures for [`super`]: several local servers holding the same file, some of
//! them broken or lying, and a ledger that records what the run told it.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rd_core::{ChecksumAlgorithm, ChunkId, PieceHashes};
use rd_limits::ScopedLimiter;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::{MultiSourceRequest, SourceEndpoint, SourceLedger};
use crate::{CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, HttpDownloadError};

const PAYLOAD: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
const PIECE: u64 = 8;

/// How a fixture server behaves.
#[derive(Clone, Copy)]
enum Behaviour {
    /// Serves the requested range correctly.
    Honest,
    /// Serves the requested range with every byte flipped.
    Corrupt,
    /// Answers every request with `503`.
    Down,
}

#[derive(Default)]
struct Seen {
    requests: AtomicUsize,
    ranges: Mutex<Vec<String>>,
}

struct Fixture {
    behaviour: Behaviour,
    seen: Arc<Seen>,
}

async fn serve_range(State(fixture): State<Arc<Fixture>>, headers: HeaderMap) -> Response {
    fixture.seen.requests.fetch_add(1, Ordering::SeqCst);
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if let Ok(mut ranges) = fixture.seen.ranges.lock() {
        ranges.push(range.clone());
    }
    if matches!(fixture.behaviour, Behaviour::Down) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Some((start, end)) = range
        .strip_prefix("bytes=")
        .and_then(|value| value.split_once('-'))
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let start = start.parse::<usize>().expect("range start");
    let end = end
        .parse::<usize>()
        .unwrap_or(PAYLOAD.len() - 1)
        .min(PAYLOAD.len() - 1);
    let mut body = PAYLOAD[start..=end].to_vec();
    if matches!(fixture.behaviour, Behaviour::Corrupt) {
        for byte in &mut body {
            *byte = !*byte;
        }
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{end}/{}", PAYLOAD.len()))
            .expect("content range"),
    );
    (StatusCode::PARTIAL_CONTENT, response_headers, body).into_response()
}

async fn origin(behaviour: Behaviour) -> (SocketAddr, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let app = Router::new()
        .route("/file", get(serve_range))
        .with_state(Arc::new(Fixture {
            behaviour,
            seen: Arc::clone(&seen),
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (address, seen)
}

fn endpoint(position: u32, address: SocketAddr) -> SourceEndpoint {
    SourceEndpoint {
        position,
        url: format!("http://{address}/file")
            .parse()
            .expect("fixture URL"),
        headers: Vec::new(),
    }
}

/// `count` equal chunks over the payload, nothing confirmed yet.
fn chunks(count: u64) -> Vec<ChunkSpec> {
    let size = PAYLOAD.len() as u64 / count;
    (0..count)
        .map(|index| ChunkSpec {
            id: ChunkId::new(),
            start: index * size,
            end: Some((index + 1) * size),
            committed: index * size,
        })
        .collect()
}

fn pieces() -> Arc<PieceHashes> {
    Arc::new(PieceHashes {
        algorithm: ChecksumAlgorithm::Sha256,
        length: PIECE,
        hashes: PAYLOAD
            .chunks(PIECE as usize)
            .map(|piece| hex::encode(Sha256::digest(piece)))
            .collect(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Entry {
    Delivered(u32, u64),
    Failed(u32, String),
    Isolated(u32, String),
    Marked(ChunkId, Option<u32>, bool),
    Rewound(ChunkId, u64),
}

#[derive(Default)]
struct Ledger {
    entries: Mutex<Vec<Entry>>,
    committed: Mutex<HashMap<ChunkId, u64>>,
}

impl Ledger {
    fn entries(&self) -> Vec<Entry> {
        self.entries.lock().expect("ledger").clone()
    }

    fn push(&self, entry: Entry) {
        self.entries.lock().expect("ledger").push(entry);
    }
}

#[async_trait]
impl CheckpointSink for Ledger {
    async fn commit(&self, chunk_id: ChunkId, committed_offset: u64) -> anyhow::Result<()> {
        self.committed
            .lock()
            .expect("ledger")
            .insert(chunk_id, committed_offset);
        Ok(())
    }
}

#[async_trait]
impl SourceLedger for Ledger {
    async fn source_delivered(&self, position: u32, bytes: u64) -> anyhow::Result<()> {
        self.push(Entry::Delivered(position, bytes));
        Ok(())
    }

    async fn source_failed(
        &self,
        position: u32,
        code: &str,
        _retry_after_seconds: Option<u64>,
    ) -> anyhow::Result<()> {
        self.push(Entry::Failed(position, code.to_owned()));
        Ok(())
    }

    async fn source_isolated(&self, position: u32, code: &str) -> anyhow::Result<()> {
        self.push(Entry::Isolated(position, code.to_owned()));
        Ok(())
    }

    async fn chunk_marked(
        &self,
        chunk_id: ChunkId,
        position: Option<u32>,
        verified: bool,
    ) -> anyhow::Result<()> {
        self.push(Entry::Marked(chunk_id, position, verified));
        Ok(())
    }

    async fn chunk_rewound(&self, chunk_id: ChunkId, committed: u64) -> anyhow::Result<()> {
        self.push(Entry::Rewound(chunk_id, committed));
        Ok(())
    }
}

fn request(
    part_path: std::path::PathBuf,
    chunks: Vec<ChunkSpec>,
    sources: Vec<SourceEndpoint>,
    pieces: Option<Arc<PieceHashes>>,
) -> MultiSourceRequest {
    MultiSourceRequest {
        part_path,
        total_bytes: PAYLOAD.len() as u64,
        chunks,
        parallel_sources: sources.len(),
        sources,
        pieces,
        unverified: HashMap::new(),
    }
}

fn engine() -> DownloadEngine {
    DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited())
}

#[tokio::test]
async fn chunks_of_one_file_are_fetched_from_two_sources_at_once() {
    let (first, first_seen) = origin(Behaviour::Honest).await;
    let (second, second_seen) = origin(Behaviour::Honest).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("two.part");
    let ledger = Arc::new(Ledger::default());

    let outcome = engine()
        .download_from_sources(
            request(
                part_path.clone(),
                chunks(4),
                vec![endpoint(0, first), endpoint(1, second)],
                Some(pieces()),
            ),
            ledger.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("a two-source download completes");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(tokio::fs::read(&part_path).await.expect("part"), PAYLOAD);
    // Both origins served chunks of the same file, and every chunk was checked.
    assert!(first_seen.requests.load(Ordering::SeqCst) >= 1);
    assert!(second_seen.requests.load(Ordering::SeqCst) >= 1);
    let entries = ledger.entries();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Marked(_, Some(_), true)))
            .count(),
        4
    );
    let delivered: u64 = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Delivered(_, bytes) => Some(*bytes),
            _ => None,
        })
        .sum();
    assert_eq!(delivered, PAYLOAD.len() as u64);
}

#[tokio::test]
async fn a_broken_mirror_hands_its_chunk_on_without_losing_confirmed_bytes() {
    let (down, _) = origin(Behaviour::Down).await;
    let (good, good_seen) = origin(Behaviour::Honest).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("failover.part");
    // An earlier run confirmed the first five bytes of the first chunk.
    let mut layout = chunks(2);
    layout[0].committed = 5;
    tokio::fs::write(&part_path, &PAYLOAD[..5])
        .await
        .expect("confirmed bytes");
    let ledger = Arc::new(Ledger::default());

    let outcome = engine()
        .download_from_sources(
            request(
                part_path.clone(),
                layout,
                vec![endpoint(0, down), endpoint(1, good)],
                None,
            ),
            ledger.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("the healthy mirror finishes the file");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(tokio::fs::read(&part_path).await.expect("part"), PAYLOAD);
    assert!(ledger.entries().iter().any(|entry| matches!(
        entry,
        Entry::Failed(0, code) if code == "download.http_status"
    )));
    // The chunk moved to the healthy mirror from the confirmed offset, not from its start.
    let ranges = good_seen.ranges.lock().expect("ranges").clone();
    assert!(
        ranges.iter().any(|range| range == "bytes=5-15"),
        "{ranges:?}"
    );
    assert!(
        !ranges.iter().any(|range| range.starts_with("bytes=0-")),
        "{ranges:?}"
    );
}

#[tokio::test]
async fn a_piece_that_does_not_match_isolates_the_mirror_that_sent_it() {
    let (liar, _) = origin(Behaviour::Corrupt).await;
    let (honest, _) = origin(Behaviour::Honest).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("isolated.part");
    let ledger = Arc::new(Ledger::default());

    let outcome = engine()
        .download_from_sources(
            request(
                part_path.clone(),
                chunks(2),
                vec![endpoint(0, liar), endpoint(1, honest)],
                Some(pieces()),
            ),
            ledger.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("the honest mirror replaces the refused bytes");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(tokio::fs::read(&part_path).await.expect("part"), PAYLOAD);
    let entries = ledger.entries();
    assert!(entries.contains(&Entry::Isolated(0, rd_core::CODE_PIECE_MISMATCH.to_owned())));
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, Entry::Rewound(_, 0)))
    );
    // Nothing the liar sent was counted as delivered.
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, Entry::Delivered(0, _)))
    );
}

#[tokio::test]
async fn wrong_bytes_from_every_source_never_complete() {
    let (liar, _) = origin(Behaviour::Corrupt).await;
    let (other_liar, _) = origin(Behaviour::Corrupt).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("refused.part");

    let result = engine()
        .download_from_sources(
            request(
                part_path,
                chunks(2),
                vec![endpoint(0, liar), endpoint(1, other_liar)],
                Some(pieces()),
            ),
            Arc::new(Ledger::default()),
            CancellationToken::new(),
        )
        .await;

    let Err(HttpDownloadError::Failure(failure)) = result else {
        panic!("a file no source delivered correctly must not complete");
    };
    assert_eq!(failure.code.as_deref(), Some(rd_core::CODE_PIECE_MISMATCH));
}

#[tokio::test]
async fn an_unchecked_chunk_from_before_a_restart_is_checked_first() {
    let (honest, honest_seen) = origin(Behaviour::Honest).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("restart.part");
    // The first chunk was completed by source 3 before the restart, with a wrong byte in its
    // second piece, and never checked.
    let mut on_disk = PAYLOAD.to_vec();
    on_disk[9] = b'!';
    tokio::fs::write(&part_path, &on_disk[..16])
        .await
        .expect("earlier bytes");
    let mut layout = chunks(2);
    layout[0].committed = 16;
    let unchecked = layout[0].id;
    let ledger = Arc::new(Ledger::default());
    let mut request = request(
        part_path.clone(),
        layout,
        vec![endpoint(0, honest)],
        Some(pieces()),
    );
    request.unverified.insert(unchecked, Some(3));

    let outcome = engine()
        .download_from_sources(request, ledger.clone(), CancellationToken::new())
        .await
        .expect("the refused piece is fetched again");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(tokio::fs::read(&part_path).await.expect("part"), PAYLOAD);
    let entries = ledger.entries();
    assert!(entries.contains(&Entry::Isolated(3, rd_core::CODE_PIECE_MISMATCH.to_owned())));
    // Back to the start of the bad piece, not of the chunk: the first piece was right.
    assert!(entries.contains(&Entry::Rewound(unchecked, PIECE)));
    let ranges = honest_seen.ranges.lock().expect("ranges").clone();
    assert!(
        ranges.iter().any(|range| range == "bytes=8-15"),
        "{ranges:?}"
    );
}

#[tokio::test]
async fn without_a_hash_basis_one_source_works_at_a_time() {
    let (first, _) = origin(Behaviour::Honest).await;
    let (second, second_seen) = origin(Behaviour::Honest).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("serial.part");
    let mut request = request(
        part_path.clone(),
        chunks(4),
        vec![endpoint(0, first), endpoint(1, second)],
        None,
    );
    request.parallel_sources = 1;

    let outcome = engine()
        .download_from_sources(
            request,
            Arc::new(Ledger::default()),
            CancellationToken::new(),
        )
        .await
        .expect("download");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(tokio::fs::read(&part_path).await.expect("part"), PAYLOAD);
    // Nothing proves the bytes, so the second mirror only stands by.
    assert_eq!(second_seen.requests.load(Ordering::SeqCst), 0);
}

/// Answers every name with loopback: a mirror whose name was rebound after it was checked.
struct Rebound;

impl crate::HostLookup for Rebound {
    fn lookup<'a>(&'a self, _host: &'a str) -> crate::LookupFuture<'a> {
        Box::pin(async { Ok(vec![std::net::IpAddr::from([127, 0, 0, 1])]) })
    }
}

#[tokio::test]
async fn a_mirror_that_points_inside_at_connect_time_is_isolated_and_the_others_finish() {
    let (rebound, rebound_seen) = origin(Behaviour::Honest).await;
    let (good, _) = origin(Behaviour::Honest).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("rebound.part");
    let ledger = Arc::new(Ledger::default());
    // The client a guarded download gets. The literal address of the second mirror never
    // reaches a resolver — before the request it is judged by `check_target`, which a test
    // on loopback cannot pass — so it stands in for a public mirror here.
    let client = reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(crate::GuardedResolver::with_lookup(
            crate::AddressPolicy::new(true),
            Arc::new(Rebound),
        ))
        .build()
        .expect("client");
    let named = SourceEndpoint {
        position: 0,
        url: format!("http://mirror.test:{}/file", rebound.port())
            .parse()
            .expect("URL"),
        headers: Vec::new(),
    };

    let outcome = DownloadEngine::new(client, ScopedLimiter::unlimited())
        .download_from_sources(
            request(
                part_path.clone(),
                chunks(2),
                vec![named, endpoint(1, good)],
                None,
            ),
            ledger.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("the other mirror finishes the file");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(tokio::fs::read(&part_path).await.expect("part"), PAYLOAD);
    let entries = ledger.entries();
    assert!(
        entries.contains(&Entry::Isolated(
            0,
            rd_core::CODE_INTERNAL_ADDRESS.to_owned()
        )),
        "{entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, Entry::Failed(0, _))),
        "{entries:?}"
    );
    assert_eq!(rebound_seen.requests.load(Ordering::SeqCst), 0);
}

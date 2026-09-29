//! [`super`] through the multi-source engine: in-memory mirrors standing in for the FTP and
//! SFTP runners, one honest, one that the address guard refuses, one that breaks off early.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{ChunkId, Failure, FailureKind, RemoteProtocol, RemoteTarget};
use rd_limits::ScopedLimiter;
use tokio_util::sync::CancellationToken;

use super::{RangeReader, RangeSource, RangeTransport};
use crate::{
    AddressPolicy, CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, MultiSourceRequest,
    SourceEndpoint, SourceLedger,
};

const PAYLOAD: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";

#[derive(Clone, Copy)]
enum Mirror {
    Honest,
    /// What a mirror whose name resolves to this machine answers when it is opened.
    Refused,
    /// Sends `n` bytes of every range, then ends the stream.
    BreaksAfter(usize),
}

struct InMemory {
    mirror: Mirror,
    opened: Mutex<Vec<u64>>,
}

#[async_trait]
impl RangeSource for InMemory {
    async fn size(
        &self,
        _target: &RemoteTarget,
        _policy: Option<&AddressPolicy>,
    ) -> Result<u64, Failure> {
        Ok(PAYLOAD.len() as u64)
    }

    async fn open_at(
        &self,
        _target: &RemoteTarget,
        offset: u64,
        _policy: Option<&AddressPolicy>,
    ) -> Result<RangeReader, Failure> {
        self.opened.lock().expect("opened").push(offset);
        let start = usize::try_from(offset).expect("offset");
        let bytes = match self.mirror {
            Mirror::Honest => PAYLOAD[start..].to_vec(),
            Mirror::Refused => {
                return Err(Failure::coded(
                    FailureKind::Permanent,
                    rd_core::CODE_INTERNAL_ADDRESS,
                    "refused",
                ));
            }
            Mirror::BreaksAfter(count) => {
                PAYLOAD[start..(start + count).min(PAYLOAD.len())].to_vec()
            }
        };
        Ok(Box::new(std::io::Cursor::new(bytes)))
    }
}

fn mirror(position: u32, behaviour: Mirror) -> (SourceEndpoint, Arc<InMemory>) {
    let source = Arc::new(InMemory {
        mirror: behaviour,
        opened: Mutex::new(Vec::new()),
    });
    let target = RemoteTarget {
        protocol: RemoteProtocol::Ftp,
        host: format!("mirror{position}.example"),
        port: 21,
        path: "/file".to_owned(),
        username: None,
        secure: false,
    };
    let endpoint = SourceEndpoint {
        position,
        url: format!("ftp://mirror{position}.example/file")
            .parse()
            .expect("url"),
        headers: Vec::new(),
        via: Some(RangeTransport {
            source: source.clone(),
            target,
            policy: Some(AddressPolicy::new(false)),
        }),
    };
    (endpoint, source)
}

#[derive(Default)]
struct Ledger {
    isolated: Mutex<Vec<(u32, String)>>,
    failed: Mutex<Vec<(u32, String)>>,
    committed: Mutex<HashMap<ChunkId, u64>>,
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
    async fn source_delivered(&self, _position: u32, _bytes: u64) -> anyhow::Result<()> {
        Ok(())
    }

    async fn source_failed(
        &self,
        position: u32,
        code: &str,
        _retry_after_seconds: Option<u64>,
    ) -> anyhow::Result<()> {
        self.failed
            .lock()
            .expect("ledger")
            .push((position, code.to_owned()));
        Ok(())
    }

    async fn source_isolated(&self, position: u32, code: &str) -> anyhow::Result<()> {
        self.isolated
            .lock()
            .expect("ledger")
            .push((position, code.to_owned()));
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

fn two_chunks() -> Vec<ChunkSpec> {
    (0..2_u64)
        .map(|index| ChunkSpec {
            id: ChunkId::new(),
            start: index * 16,
            end: Some((index + 1) * 16),
            committed: index * 16,
        })
        .collect()
}

async fn fetch(
    sources: Vec<SourceEndpoint>,
    ledger: Arc<Ledger>,
) -> (DownloadOutcome, Vec<u8>, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("file.part");
    let parallel = sources.len();
    let outcome = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited())
        .download_from_sources(
            MultiSourceRequest {
                part_path: part_path.clone(),
                total_bytes: PAYLOAD.len() as u64,
                chunks: two_chunks(),
                sources,
                parallel_sources: parallel,
                pieces: None,
                unverified: HashMap::new(),
            },
            ledger,
            CancellationToken::new(),
        )
        .await
        .expect("download");
    let bytes = tokio::fs::read(&part_path).await.expect("part");
    (outcome, bytes, directory)
}

/// An FTP mirror the guard refused when it was opened is isolated with the stable code; the
/// honest one delivers every chunk, and the file is exact.
#[tokio::test]
async fn a_refused_ftp_mirror_is_isolated_and_the_other_delivers_the_file() {
    let (refused, refused_source) = mirror(0, Mirror::Refused);
    let (honest, honest_source) = mirror(1, Mirror::Honest);
    let ledger = Arc::new(Ledger::default());

    let (outcome, bytes, _directory) = fetch(vec![refused, honest], Arc::clone(&ledger)).await;

    assert!(matches!(outcome, DownloadOutcome::Complete));
    assert_eq!(bytes, PAYLOAD);
    assert_eq!(
        *ledger.isolated.lock().expect("ledger"),
        vec![(0, rd_core::CODE_INTERNAL_ADDRESS.to_owned())]
    );
    assert!(!refused_source.opened.lock().expect("opened").is_empty());
    assert!(!honest_source.opened.lock().expect("opened").is_empty());
}

/// A mirror that breaks off mid-chunk keeps what it delivered: the next source is opened at
/// the confirmed offset, not at the chunk's start.
#[tokio::test]
async fn a_mirror_that_breaks_off_hands_on_from_the_confirmed_offset() {
    let (broken, _) = mirror(0, Mirror::BreaksAfter(5));
    let (honest, honest_source) = mirror(1, Mirror::Honest);
    let ledger = Arc::new(Ledger::default());

    let (outcome, bytes, _directory) = fetch(vec![broken, honest], Arc::clone(&ledger)).await;

    assert!(matches!(outcome, DownloadOutcome::Complete));
    assert_eq!(bytes, PAYLOAD);
    assert!(
        ledger
            .failed
            .lock()
            .expect("ledger")
            .iter()
            .any(|(position, code)| *position == 0 && code == "download.response_truncated")
    );
    let opened = honest_source.opened.lock().expect("opened").clone();
    // The broken mirror had the first chunk and delivered five of its bytes before it ended.
    assert!(opened.contains(&5), "{opened:?}");
}

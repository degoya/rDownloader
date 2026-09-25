//! Fixtures shared by the crash, pipelining and throughput tests.
//!
//! The scripted NNTP server here differs from the one in `worker_tests` in two ways that
//! matter for RD-108-25: it reads commands and writes responses in separate tasks, so a
//! client that pipelines a second `BODY` sees it accepted while the first body is still on
//! its way, and it can add a round-trip delay and a per-connection byte rate, which is what
//! turns a loopback socket into something a throughput number can be read off.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use rd_core::{NzbFileStatus, NzbSegmentState};
use rd_db::{Database, NewNzbFile, NewNzbImport, NewNzbSegment};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig,
    worker::{FileOutcome, download_file},
};

/// yEnc-encodes `payload` into 128-column lines, dot-stuffed, escape pairs never split.
pub(crate) fn yenc_lines(payload: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(payload.len() + payload.len() / 64 + 16);
    let mut column = 0_usize;
    for byte in payload {
        let shifted = byte.wrapping_add(42);
        let pair = matches!(shifted, 0 | 10 | 13 | 61);
        if column == 0 && shifted == b'.' && !pair {
            encoded.push(b'.');
        }
        if pair {
            encoded.push(b'=');
            encoded.push(shifted.wrapping_add(64));
            column += 2;
        } else {
            encoded.push(shifted);
            column += 1;
        }
        if column >= 128 {
            encoded.extend_from_slice(b"\r\n");
            column = 0;
        }
    }
    if column > 0 {
        encoded.extend_from_slice(b"\r\n");
    }
    encoded
}

/// One `222` response carrying part `number` of `name`, `payload` starting at byte `begin` (1-based).
pub(crate) fn multipart_article(
    name: &str,
    number: u64,
    total: u64,
    begin: u64,
    payload: &[u8],
) -> Vec<u8> {
    let end = begin + payload.len() as u64 - 1;
    let mut article = format!(
        "222 body follows\r\n=ybegin part={number} line=128 size={total} name={name}\r\n=ypart begin={begin} end={end}\r\n"
    )
    .into_bytes();
    article.extend(yenc_lines(payload));
    article.extend(
        format!(
            "=yend size={} part={number} pcrc32={:08x}\r\n.\r\n",
            payload.len(),
            crc32fast::hash(payload)
        )
        .as_bytes(),
    );
    article
}

/// What the fixture saw: every message id in the order its `BODY` arrived, per connection.
#[derive(Default)]
pub(crate) struct FixtureLog {
    pub connections: Mutex<Vec<Vec<String>>>,
    /// Answers the fixture sent out of order on purpose (`swaps_pipelined_answers`).
    pub swapped: std::sync::atomic::AtomicUsize,
}

impl FixtureLog {
    /// How often two queued commands were answered in the wrong order. A swapping fixture
    /// that never got two commands queued at once has not played its part; a test should
    /// say so rather than fail later on what it expected the swap to cause.
    pub(crate) fn swapped(&self) -> usize {
        self.swapped.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn requests(&self) -> Vec<String> {
        self.connections
            .lock()
            .expect("fixture log")
            .iter()
            .flatten()
            .cloned()
            .collect()
    }

    pub(crate) fn connection_count(&self) -> usize {
        self.connections.lock().expect("fixture log").len()
    }
}

/// How the fixture answers: `rtt` elapses between a command and the first byte of its
/// answer, `bytes_per_second` paces the answer on that one connection.
#[derive(Clone, Copy, Default)]
pub(crate) struct FixtureTiming {
    pub rtt: Duration,
    pub bytes_per_second: Option<u64>,
}

/// Scripted articles; a `None` answers `430`.
pub(crate) type Articles = Arc<HashMap<String, Option<Vec<u8>>>>;

/// Answers a server gives before it gives the article (RD-108-29).
///
/// Counted per message id, so every article sees the same run of bad answers however the
/// requests are spread over connections: first `transient` refusals, then `corrupt` bodies,
/// then the article itself.
#[derive(Clone, Default)]
pub(crate) struct Faults {
    /// `400 Archive server temporarily offline.` answers - the field shape of RD-108-29.
    pub transient: u32,
    /// Bodies whose `pcrc32` does not match, the way a truncated article reads.
    pub corrupt: u32,
    attempts: Arc<Mutex<HashMap<String, u32>>>,
}

impl Faults {
    /// `times` refusals for every article before the real body.
    pub(crate) fn transient(times: u32) -> Self {
        Self {
            transient: times,
            ..Self::default()
        }
    }

    /// `times` broken bodies for every article before the real body.
    pub(crate) fn corrupt(times: u32) -> Self {
        Self {
            corrupt: times,
            ..Self::default()
        }
    }

    /// What this request gets, counting the ones that came before it for the same id.
    fn next(&self, message_id: &str) -> Answer {
        let mut attempts = self.attempts.lock().expect("fixture faults");
        let attempt = attempts.entry(message_id.to_owned()).or_insert(0);
        let seen = *attempt;
        *attempt += 1;
        if seen < self.transient {
            Answer::Transient
        } else if seen < self.transient.saturating_add(self.corrupt) {
            Answer::Corrupt
        } else {
            Answer::Article
        }
    }
}

enum Answer {
    Transient,
    Corrupt,
    Article,
}

/// One article the fixture holds back until the test lets it through (RD-108-26).
///
/// A server that is slow with one article and quick with the rest, made deterministic: the
/// answer waits, so a client that cannot get past it stops asking for anything else.
#[derive(Clone)]
pub(crate) struct Gate {
    message_id: String,
    open: Arc<tokio::sync::Semaphore>,
}

impl Gate {
    pub(crate) fn on(message_id: &str) -> Self {
        Self {
            message_id: message_id.to_owned(),
            open: Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }

    /// Lets the held article through.
    pub(crate) fn open(&self) {
        self.open.add_permits(1);
    }

    async fn wait_for(&self, message_id: &str) {
        if message_id != self.message_id {
            return;
        }
        if let Ok(permit) = self.open.clone().acquire_owned().await {
            permit.forget();
        }
    }
}

/// The status a server answers when its backend is not reachable right now. Copied from the
/// live log that RD-108-29 came from, down to the wording.
pub(crate) const TRANSIENT_STATUS: &str = "400 Archive server temporarily offline.";

/// What kind of server the fixture plays (RD-108-27).
#[derive(Clone)]
pub(crate) struct FixtureBehaviour {
    /// The `222` line names the message-id, as RFC 3977 has it and real servers do.
    pub names_message_id: bool,
    /// The `222` line names the requested id in another spelling (upper case): a server
    /// whose ids cannot be checked, which must not cost a single article.
    pub spells_id_differently: bool,
    /// Two commands queued at once are answered in the wrong order - the field shape of
    /// RD-108-27, where the earlier request on a line read the later one's body.
    pub swaps_pipelined_answers: bool,
    /// Bad answers before the good one (RD-108-29).
    pub faults: Faults,
    /// One article the fixture answers only once the test says so (RD-108-26).
    pub gate: Option<Gate>,
}

impl FixtureBehaviour {
    /// The in-order server of RD-108-27, answering `faults` before every article.
    pub(crate) fn with(faults: Faults) -> Self {
        Self {
            faults,
            ..Self::default()
        }
    }
}

impl Default for FixtureBehaviour {
    fn default() -> Self {
        Self {
            names_message_id: true,
            spells_id_differently: false,
            swaps_pipelined_answers: false,
            faults: Faults::default(),
            gate: None,
        }
    }
}

/// Serves `articles` on any number of connections, answering in command order.
///
/// Commands are read by one task and answered by another, so a second `BODY` is taken off
/// the socket while the first body is still being written - the shape a real server has and
/// the one a pipelining client depends on.
pub(crate) async fn spawn_fixture(
    articles: Articles,
    timing: FixtureTiming,
) -> (SocketAddr, Arc<FixtureLog>) {
    spawn_fixture_with(articles, timing, FixtureBehaviour::default()).await
}

/// A `222` answer with the message-id on its status line, the way a real server sends it.
pub(crate) fn named_answer(reply: Vec<u8>, message_id: &str) -> Vec<u8> {
    let Some(end) = reply.windows(2).position(|pair| pair == b"\r\n") else {
        return reply;
    };
    if !reply.starts_with(b"222") {
        return reply;
    }
    let mut named = format!("222 0 <{message_id}> body follows\r\n").into_bytes();
    named.extend_from_slice(&reply[end + 2..]);
    named
}

/// [`spawn_fixture`] with the server's behaviour chosen.
pub(crate) async fn spawn_fixture_with(
    articles: Articles,
    timing: FixtureTiming,
    behaviour: FixtureBehaviour,
) -> (SocketAddr, Arc<FixtureLog>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let log = Arc::new(FixtureLog::default());
    let served = Arc::clone(&log);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let index = {
                let mut connections = served.connections.lock().expect("fixture log");
                connections.push(Vec::new());
                connections.len() - 1
            };
            let (read, mut write) = stream.into_split();
            let (queue, mut answers) = tokio::sync::mpsc::unbounded_channel();
            let articles = Arc::clone(&articles);
            let log = Arc::clone(&served);
            let swaps = Arc::clone(&served);
            let reading = behaviour.clone();
            let writing = behaviour.clone();
            tokio::spawn(async move {
                let mut read = BufReader::new(read);
                loop {
                    let mut command = String::new();
                    if read.read_line(&mut command).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let requested = command
                        .trim()
                        .strip_prefix("BODY <")
                        .and_then(|rest| rest.strip_suffix('>'))
                        .expect("bracketed BODY command")
                        .to_owned();
                    log.connections.lock().expect("fixture log")[index].push(requested.clone());
                    let mut reply = articles
                        .get(&requested)
                        .cloned()
                        .unwrap_or_else(|| panic!("unscripted message id {requested}"))
                        .unwrap_or_else(|| b"430 No such article\r\n".to_vec());
                    // A `400` is the server saying it cannot serve right now; RFC 3977
                    // §3.2.1 has it close the connection afterwards, and so does this.
                    let mut closes = false;
                    match reading.faults.next(&requested) {
                        Answer::Transient => {
                            reply = format!("{TRANSIENT_STATUS}\r\n").into_bytes();
                            closes = true;
                        }
                        Answer::Corrupt => reply = corrupt_checksum(reply),
                        Answer::Article => {}
                    }
                    if !closes {
                        if reading.spells_id_differently {
                            reply = named_answer(reply, &requested.to_uppercase());
                        } else if reading.names_message_id {
                            reply = named_answer(reply, &requested);
                        }
                    }
                    if let Some(gate) = &reading.gate {
                        gate.wait_for(&requested).await;
                    }
                    if queue
                        .send((tokio::time::Instant::now() + timing.rtt, reply, closes))
                        .is_err()
                    {
                        return;
                    }
                }
            });
            tokio::spawn(async move {
                // A connection is not free: TCP, TLS and the login are a round trip each
                // before the greeting is of any use. Modelled as three, so a measurement can
                // see what opening one costs (RD-108-26).
                tokio::time::sleep(timing.rtt * 3).await;
                if write.write_all(b"200 fixture ready\r\n").await.is_err() {
                    return;
                }
                while let Some((due, reply, closes)) = answers.recv().await {
                    tokio::time::sleep_until(due).await;
                    // The server of RD-108-27: with a second command already waiting, its
                    // answer goes out first.
                    let ordered = match writing
                        .swaps_pipelined_answers
                        .then(|| answers.try_recv().ok())
                        .flatten()
                    {
                        Some((_, second, _)) => {
                            swaps
                                .swapped
                                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                            vec![second, reply]
                        }
                        None => vec![reply],
                    };
                    for reply in ordered {
                        if write_paced(&mut write, &reply, timing.bytes_per_second)
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    if closes {
                        return;
                    }
                }
            });
        }
    });
    (address, log)
}

/// The same article with a `pcrc32` nothing hashes to: what a truncated body reads like.
///
/// Byte work, not text work: the payload between the control lines is binary and rarely
/// valid UTF-8.
fn corrupt_checksum(mut reply: Vec<u8>) -> Vec<u8> {
    const MARKER: &[u8] = b"pcrc32=";
    let Some(start) = reply
        .windows(MARKER.len())
        .position(|window| window == MARKER)
    else {
        return reply;
    };
    let digits = start + MARKER.len();
    reply[digits..digits + 8].copy_from_slice(b"deadbeef");
    reply
}

async fn write_paced(
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    reply: &[u8],
    bytes_per_second: Option<u64>,
) -> std::io::Result<()> {
    let Some(rate) = bytes_per_second else {
        return write.write_all(reply).await;
    };
    const CHUNK: usize = 64 * 1024;
    let started = tokio::time::Instant::now();
    let mut sent = 0_usize;
    for chunk in reply.chunks(CHUNK) {
        write.write_all(chunk).await?;
        sent += chunk.len();
        let due = started + Duration::from_secs_f64(sent as f64 / rate as f64);
        tokio::time::sleep_until(due).await;
    }
    Ok(())
}

/// Imports one NZB file with the given `(message_id, bytes)` segments and returns its row.
pub(crate) async fn import_single_file(
    database: &Database,
    subject: &str,
    segments: &[(String, u64)],
) -> NzbFileStatus {
    let import = database
        .add_nzb_import(NewNzbImport {
            name: format!("{subject}.nzb"),
            sha256: "ab".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: rd_core::ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: subject.to_owned(),
                poster: "fixture".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: segments
                    .iter()
                    .enumerate()
                    .map(|(index, (message_id, bytes))| NewNzbSegment {
                        number: u32::try_from(index + 1).expect("segment number"),
                        bytes: *bytes,
                        message_id: message_id.clone(),
                    })
                    .collect(),
            }],
        })
        .await
        .expect("NZB import");
    database
        .list_nzb_files(import.id)
        .await
        .expect("files")
        .remove(0)
}

/// Re-reads the row after checkpoints changed it.
pub(crate) async fn reload(database: &Database, file: &NzbFileStatus) -> NzbFileStatus {
    database
        .list_nzb_files(file.import_id)
        .await
        .expect("files")
        .into_iter()
        .find(|candidate| candidate.id == file.id)
        .expect("file row")
}

/// A payload whose bytes never repeat with a power-of-two period, so a resume that lands on
/// the wrong offset produces different bytes rather than identical ones.
pub(crate) fn payload(length: usize, seed: usize) -> Vec<u8> {
    (0..length)
        .map(|index| ((index + seed) % 251) as u8)
        .collect()
}

pub(crate) fn run_limits() -> rd_scheduler::RunLimits {
    rd_scheduler::RunLimits {
        max_parallel_requests: 32,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
    }
}

pub(crate) const SEGMENT_BYTES: usize = 3000;

pub(crate) struct Set {
    pub articles: Articles,
    pub segments: Vec<(String, u64)>,
    /// The file as it should end up: the articles in order, a zero hole per missing one.
    pub expected: Vec<u8>,
}

pub(crate) fn article_set(count: usize, missing: &[usize]) -> Set {
    let total = (count * SEGMENT_BYTES) as u64;
    let mut articles = HashMap::new();
    let mut segments = Vec::with_capacity(count);
    let mut expected = Vec::with_capacity(count * SEGMENT_BYTES);
    for index in 0..count {
        let number = index + 1;
        let message_id = format!("part-{number}@example.test");
        let bytes = payload(SEGMENT_BYTES, index * 13);
        let begin = (index * SEGMENT_BYTES) as u64 + 1;
        if missing.contains(&number) {
            articles.insert(message_id.clone(), None);
            expected.extend(std::iter::repeat_n(0_u8, SEGMENT_BYTES));
        } else {
            articles.insert(
                message_id.clone(),
                Some(multipart_article(
                    "file.bin",
                    number as u64,
                    total,
                    begin,
                    &bytes,
                )),
            );
            expected.extend_from_slice(&bytes);
        }
        segments.push((message_id, SEGMENT_BYTES as u64));
    }
    Set {
        articles: Arc::new(articles),
        segments,
        expected,
    }
}

pub(crate) struct Run {
    pub outcome: anyhow::Result<FileOutcome>,
    pub database: Database,
    pub file: NzbFileStatus,
    pub pool: NntpPool,
    pub log: Arc<FixtureLog>,
    _directory: tempfile::TempDir,
}

/// One file through the worker path against the fixture playing `behaviour`. The round trip
/// is what makes the second command arrive before the first answer is due, every time.
pub(crate) async fn run(set: &Set, connections: u16, behaviour: FixtureBehaviour) -> Run {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("pipelining.sqlite"))
        .await
        .expect("database");
    let file = import_single_file(&database, "file.bin", &set.segments).await;
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let (address, log) = spawn_fixture_with(
        Arc::clone(&set.articles),
        FixtureTiming {
            rtt: Duration::from_millis(40),
            bytes_per_second: None,
        },
        behaviour,
    )
    .await;
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 64 * 1024,
        max_connections: connections,
    }])
    .expect("pool");
    let outcome = download_file(
        &database,
        &pool,
        &CancellationToken::new(),
        &file,
        &staging,
        &destination,
        &run_limits(),
    )
    .await;
    Run {
        outcome,
        database,
        file,
        pool,
        log,
        _directory: directory,
    }
}

pub(crate) fn completed(outcome: anyhow::Result<FileOutcome>) -> (std::path::PathBuf, usize) {
    match outcome.expect("download") {
        FileOutcome::Completed { path, missing } => (path, missing),
        FileOutcome::Cancelled => panic!("download was cancelled"),
    }
}

pub(crate) async fn states(database: &Database, file: &NzbFileStatus) -> Vec<NzbSegmentState> {
    let mut segments = reload(database, file).await.segments;
    segments.sort_by_key(|segment| segment.number);
    segments.into_iter().map(|segment| segment.state).collect()
}

//! The answers a resumed HTTP chunk must refuse, against a fake origin (RD-1190-01).
//!
//! A part file holds the first bytes of the payload; each origin answers the ranged request
//! for the rest one way. A refusal leaves the part file exactly as it was: the head intact,
//! nothing written behind it. TR-04 pins the refusals the engine already makes (412, 416, a
//! `200` after `If-Range`, a `206` for another range); TR-01 adds the length: a `Content-Range`
//! naming another total is a different file, and without a validator only a confirmed equal
//! length may be continued.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::get,
};
use rd_core::ChunkId;
use rd_http::{
    CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
};
use rd_limits::ScopedLimiter;
use tokio_util::sync::CancellationToken;

const PAYLOAD: &[u8] = b"resume-refusal-payload-0123456789";
/// Bytes already on disk when the resume starts.
const COMMITTED: usize = 8;
const VALIDATOR: &str = "\"v1\"";

struct NoopCheckpoint;

#[async_trait::async_trait]
impl CheckpointSink for NoopCheckpoint {
    async fn commit(&self, _chunk_id: ChunkId, _committed_offset: u64) -> anyhow::Result<()> {
        Ok(())
    }
}

/// How the origin answers, and the `If-Range` each request carried.
struct Origin {
    answer: fn() -> Response,
    if_range: Mutex<Vec<Option<String>>>,
}

async fn answer(State(origin): State<Arc<Origin>>, headers: HeaderMap) -> Response {
    origin.if_range.lock().expect("if-range").push(
        headers
            .get(header::IF_RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    (origin.answer)()
}

fn respond(status: StatusCode, content_range: Option<&str>, body: &[u8]) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream");
    if let Some(range) = content_range {
        response = response.header(header::CONTENT_RANGE, range);
    }
    response
        .body(Body::from(body.to_vec()))
        .expect("fixture response")
}

/// What one resume against `answer` ended with: the result, the part file afterwards and
/// the `If-Range` the origin saw.
struct Resumed {
    result: Result<DownloadOutcome, HttpDownloadError>,
    part: Vec<u8>,
    if_range: Vec<Option<String>>,
}

async fn resume(answer_with: fn() -> Response, validator: Option<&str>) -> Resumed {
    let origin = Arc::new(Origin {
        answer: answer_with,
        if_range: Mutex::new(Vec::new()),
    });
    let address = serve(
        Router::new()
            .route("/file", get(answer))
            .with_state(Arc::clone(&origin)),
    )
    .await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("resume.part");
    tokio::fs::write(&part_path, &PAYLOAD[..COMMITTED])
        .await
        .expect("write the confirmed head");
    let total = PAYLOAD.len() as u64;
    let request = DownloadRequest {
        etag: validator.map(str::to_owned),
        use_ranges: true,
        ..DownloadRequest::get(
            format!("http://{address}/file").parse().expect("url"),
            part_path.clone(),
            Some(total),
            vec![ChunkSpec {
                id: ChunkId::new(),
                start: 0,
                end: Some(total),
                committed: COMMITTED as u64,
            }],
        )
    };
    let result = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited())
        .download(request, Arc::new(NoopCheckpoint), CancellationToken::new())
        .await;
    let part = tokio::fs::read(&part_path).await.expect("read part file");
    let if_range = origin.if_range.lock().expect("if-range").clone();
    Resumed {
        result,
        part,
        if_range,
    }
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    address
}

/// The confirmed head is intact and not one byte was written behind it.
fn untouched(part: &[u8]) -> bool {
    part[..COMMITTED] == PAYLOAD[..COMMITTED] && part[COMMITTED..].iter().all(|byte| *byte == 0)
}

fn sent_validator(resumed: &Resumed) -> bool {
    resumed.if_range == vec![Some(VALIDATOR.to_owned())]
}

// TR-04: the refusals of a conditional resume.

/// `If-Range` with a validator the origin no longer has, answered with 412.
#[tokio::test]
async fn a_412_to_a_conditional_resume_is_a_changed_remote() {
    let resumed = resume(
        || respond(StatusCode::PRECONDITION_FAILED, None, b""),
        Some(VALIDATOR),
    )
    .await;

    assert!(
        matches!(resumed.result, Err(HttpDownloadError::RemoteChanged)),
        "{:?}",
        resumed.result
    );
    assert!(sent_validator(&resumed), "{:?}", resumed.if_range);
    assert!(untouched(&resumed.part));
}

/// A file that shrank below the confirmed offset: 416 with the new length.
#[tokio::test]
async fn a_416_to_a_resume_is_a_changed_remote() {
    let resumed = resume(
        || respond(StatusCode::RANGE_NOT_SATISFIABLE, Some("bytes */4"), b""),
        Some(VALIDATOR),
    )
    .await;

    assert!(
        matches!(resumed.result, Err(HttpDownloadError::RemoteChanged)),
        "{:?}",
        resumed.result
    );
    assert!(untouched(&resumed.part));
}

/// RFC 9110: a stale `If-Range` validator gets the whole entity with `200`. With bytes
/// already on disk that is a changed file, never a body to write from offset 0 over them.
#[tokio::test]
async fn a_full_body_after_if_range_is_a_changed_remote() {
    let resumed = resume(|| respond(StatusCode::OK, None, PAYLOAD), Some(VALIDATOR)).await;

    assert!(
        matches!(resumed.result, Err(HttpDownloadError::RemoteChanged)),
        "{:?}",
        resumed.result
    );
    assert!(sent_validator(&resumed), "{:?}", resumed.if_range);
    assert!(untouched(&resumed.part));
}

/// A `206` that describes another range than the one asked for: its body belongs somewhere
/// else, and writing it at the confirmed offset is the silent corruption the header exists
/// to prevent.
#[tokio::test]
async fn a_206_for_another_range_is_refused() {
    let resumed = resume(
        || respond(StatusCode::PARTIAL_CONTENT, Some("bytes 0-32/33"), PAYLOAD),
        Some(VALIDATOR),
    )
    .await;

    assert!(
        matches!(resumed.result, Err(HttpDownloadError::RangeIgnored)),
        "{:?}",
        resumed.result
    );
    assert!(untouched(&resumed.part));
}

// TR-01: the length a described range names.

/// The requested range, from a file of another length. Before TR-01 only the start was
/// compared, so the tail of a different file completed this one.
#[tokio::test]
async fn a_206_naming_another_total_is_a_changed_remote() {
    let resumed = resume(
        || {
            respond(
                StatusCode::PARTIAL_CONTENT,
                Some("bytes 8-32/40"),
                &PAYLOAD[COMMITTED..],
            )
        },
        Some(VALIDATOR),
    )
    .await;

    assert!(
        matches!(resumed.result, Err(HttpDownloadError::RemoteChanged)),
        "{:?}",
        resumed.result
    );
    assert!(untouched(&resumed.part));
}

/// A `200` that describes the requested range is read like a `206`, the total included.
#[tokio::test]
async fn a_described_200_naming_another_total_is_a_changed_remote() {
    let resumed = resume(
        || respond(StatusCode::OK, Some("bytes 8-32/40"), &PAYLOAD[COMMITTED..]),
        Some(VALIDATOR),
    )
    .await;

    assert!(
        matches!(resumed.result, Err(HttpDownloadError::RemoteChanged)),
        "{:?}",
        resumed.result
    );
    assert!(untouched(&resumed.part));
}

/// Without `ETag` or `Last-Modified` no `If-Range` goes out, so nothing but the length can
/// tell the origin's file from the one on disk. A total the origin does not name confirms
/// nothing.
#[tokio::test]
async fn without_a_validator_an_unconfirmed_length_is_not_resumed() {
    let resumed = resume(
        || {
            respond(
                StatusCode::PARTIAL_CONTENT,
                Some("bytes 8-32/*"),
                &PAYLOAD[COMMITTED..],
            )
        },
        None,
    )
    .await;

    assert!(
        matches!(resumed.result, Err(HttpDownloadError::RemoteChanged)),
        "{:?}",
        resumed.result
    );
    assert_eq!(resumed.if_range, vec![None]);
    assert!(untouched(&resumed.part));
}

/// The same length without a validator is still a resume: the rule takes away only what
/// nothing confirms.
#[tokio::test]
async fn without_a_validator_the_same_length_resumes() {
    let resumed = resume(
        || {
            respond(
                StatusCode::PARTIAL_CONTENT,
                Some("bytes 8-32/33"),
                &PAYLOAD[COMMITTED..],
            )
        },
        None,
    )
    .await;

    assert!(
        matches!(resumed.result, Ok(DownloadOutcome::Complete)),
        "{:?}",
        resumed.result
    );
    assert_eq!(resumed.part, PAYLOAD);
}

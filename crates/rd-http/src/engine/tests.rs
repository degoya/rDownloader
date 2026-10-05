use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
};
use rd_core::{ChunkId, FailureKind};
use reqwest::{StatusCode, header};
use tokio_util::sync::CancellationToken;

use super::{
    CheckpointSink, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
    status_failure,
};
use rd_limits::ScopedLimiter;

use crate::{ChunkSpec, hostlimit::HostLimits};

struct NoopCheckpoint;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

/// A single whole-file chunk, the shape a file below the chunk size is planned as.
fn whole_file(total: u64, committed: u64) -> Vec<ChunkSpec> {
    vec![ChunkSpec {
        id: ChunkId::new(),
        start: 0,
        end: Some(total),
        committed,
    }]
}

/// A ranged request carrying a validator, as the worker always sends one.
fn ranged_request(
    address: SocketAddr,
    part_path: std::path::PathBuf,
    total: u64,
    chunks: Vec<ChunkSpec>,
) -> DownloadRequest {
    DownloadRequest {
        url: format!("http://{address}/file")
            .parse()
            .expect("fixture URL"),
        part_path,
        total_bytes: Some(total),
        etag: Some("\"fixture\"".to_owned()),
        last_modified: None,
        use_ranges: true,
        chunks,
        headers: Vec::new(),
        method: rd_core::ReplayMethod::Get,
        body: None,
        approved_origins: Arc::new(Vec::new()),
        captured_user_agent: None,
        transform: None,
    }
}

/// Answers with the complete body and status 200, as a hoster that does not implement
/// ranges does.
///
/// Built rather than assembled from a tuple, because a response part appends to the
/// content type axum derives from the body instead of replacing it, and this fixture
/// exists precisely to control that header.
fn full_body(payload: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(payload.to_vec()))
        .expect("fixture response")
}

async fn ranged_fixture(headers: HeaderMap) -> Response {
    const PAYLOAD: &[u8] = b"parallel-range-payload";
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.split_once('-'));
    let Some((start, end)) = range else {
        return (StatusCode::OK, PAYLOAD).into_response();
    };
    let start = start.parse::<usize>().expect("range start");
    let end = end.parse::<usize>().expect("range end");
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{end}/{}", PAYLOAD.len()))
            .expect("content range"),
    );
    (
        StatusCode::PARTIAL_CONTENT,
        response_headers,
        PAYLOAD[start..=end].to_vec(),
    )
        .into_response()
}

#[async_trait]
impl CheckpointSink for NoopCheckpoint {
    async fn commit(&self, _chunk_id: ChunkId, _committed_offset: u64) -> Result<()> {
        Ok(())
    }
}

/// RFC 9110 requires a `200` once an `If-Range` validator no longer matches, so the
/// full body is the server saying "this is a different file now" -- not that it ignored
/// the range. Either way the bytes already on disk must survive untouched.
#[tokio::test]
async fn a_full_response_to_resume_reports_a_changed_remote_and_keeps_the_part_file() {
    let address = serve(Router::new().route(
        "/file",
        get(|| async { full_body(b"replacement", "application/octet-stream") }),
    ))
    .await;

    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("payload.part");
    tokio::fs::write(&part_path, b"safe")
        .await
        .expect("write existing checkpoint");
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());
    let result = engine
        .download(
            ranged_request(address, part_path.clone(), 4, whole_file(4, 2)),
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await;

    assert!(matches!(result, Err(HttpDownloadError::RemoteChanged)));
    assert_eq!(
        tokio::fs::read(part_path).await.expect("read part file"),
        b"safe"
    );
}

/// The reported defect: a small file is planned as one chunk, the probe saw
/// `Accept-Ranges`, and the hoster answers the ranged `GET` with the whole file and a
/// plain `200`. That is a complete download, and used to be a permanent failure.
#[tokio::test]
async fn a_full_body_on_a_whole_file_chunk_is_written_from_the_start() {
    const PAYLOAD: &[u8] = b"whole-file-payload";
    let address = serve(Router::new().route(
        "/file",
        get(|| async { full_body(PAYLOAD, "application/octet-stream") }),
    ))
    .await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("whole.part");
    let total = PAYLOAD.len() as u64;
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

    let outcome = engine
        .download(
            ranged_request(address, part_path.clone(), total, whole_file(total, 0)),
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await
        .expect("a complete body is a complete download");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(
        tokio::fs::read(part_path).await.expect("read part file"),
        PAYLOAD
    );
}

/// A throttle notice or landing page carries a `200` too. It is not the file, but it is
/// also not permanent: the same link works minutes later, so it has to be retryable.
#[tokio::test]
async fn a_page_instead_of_the_payload_is_retryable_and_translatable() {
    const PAGE: &[u8] = b"<html><body>please wait 30 minutes</body></html>";
    let address = serve(Router::new().route(
        "/file",
        get(|| async { full_body(PAGE, "text/html; charset=utf-8") }),
    ))
    .await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("page.part");
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

    let result = engine
        .download(
            ranged_request(address, part_path.clone(), 4096, whole_file(4096, 0)),
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await;

    let Err(HttpDownloadError::Failure(failure)) = result else {
        panic!("a served page must be a classified failure");
    };
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: None
        }
    );
    assert_eq!(failure.code.as_deref(), Some("download.not_a_file"));
    assert_eq!(
        failure.params.get("content_type").map(String::as_str),
        Some("text/html; charset=utf-8")
    );
    // Nothing of the page may reach the part file, which is preallocated and therefore
    // all zeroes until a byte of payload is written.
    let written = tokio::fs::read(part_path).await.expect("part file");
    assert!(
        written.iter().all(|byte| *byte == 0),
        "the served page was written to the part file"
    );
}

/// `Content-Range` is the header that says what a body contains. A server that answers
/// `200` while describing the requested part is serving that part, and the resume
/// continues at the committed offset instead of being thrown away.
#[tokio::test]
async fn a_described_range_is_accepted_even_with_status_200() {
    const PAYLOAD: &[u8] = b"content-range-payload";
    const COMMITTED: usize = 8;
    let address = serve(Router::new().route(
        "/file",
        get(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {COMMITTED}-{}/{}", PAYLOAD.len() - 1, PAYLOAD.len()),
                )
                .body(Body::from(PAYLOAD[COMMITTED..].to_vec()))
                .expect("fixture response")
        }),
    ))
    .await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("described.part");
    tokio::fs::write(&part_path, &PAYLOAD[..COMMITTED])
        .await
        .expect("write existing checkpoint");
    let total = PAYLOAD.len() as u64;
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

    let outcome = engine
        .download(
            ranged_request(
                address,
                part_path.clone(),
                total,
                whole_file(total, COMMITTED as u64),
            ),
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await
        .expect("a described range completes the file");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(
        tokio::fs::read(part_path).await.expect("read part file"),
        PAYLOAD
    );
}

/// Nothing used to bound how many connections one host saw: four chunks of a file, and
/// as many files as the queue allowed, all at once.
#[tokio::test]
async fn the_host_limit_bounds_the_connections_one_host_sees() {
    const PAYLOAD: &[u8] = b"parallel-range-payload";
    #[derive(Default)]
    struct Concurrency {
        open: AtomicUsize,
        peak: AtomicUsize,
    }
    async fn counted(State(seen): State<Arc<Concurrency>>, headers: HeaderMap) -> Response {
        let open = seen.open.fetch_add(1, Ordering::SeqCst) + 1;
        seen.peak.fetch_max(open, Ordering::SeqCst);
        // Long enough that unlimited chunks would demonstrably overlap.
        tokio::time::sleep(Duration::from_millis(150)).await;
        seen.open.fetch_sub(1, Ordering::SeqCst);
        ranged_fixture(headers).await
    }

    let seen = Arc::new(Concurrency::default());
    let address = serve(
        Router::new()
            .route("/file", get(counted))
            .with_state(Arc::clone(&seen)),
    )
    .await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("limited.part");
    let total = PAYLOAD.len() as u64;
    let bounds = [0_u64, 6, 12, 17, total];
    let chunks = bounds
        .windows(2)
        .map(|pair| ChunkSpec {
            id: ChunkId::new(),
            start: pair[0],
            end: Some(pair[1]),
            committed: pair[0],
        })
        .collect::<Vec<_>>();
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited())
        .with_host_limits(HostLimits::new(2));

    let outcome = engine
        .download(
            ranged_request(address, part_path.clone(), total, chunks),
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await
        .expect("a limited download still completes");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(
        tokio::fs::read(part_path).await.expect("read part file"),
        PAYLOAD
    );
    assert!(
        seen.peak.load(Ordering::SeqCst) <= 2,
        "the host saw {} simultaneous connections, the limit was 2",
        seen.peak.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn parallel_ranges_reconstruct_the_exact_file() {
    const PAYLOAD: &[u8] = b"parallel-range-payload";
    let app = Router::new().route("/file", get(ranged_fixture));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let directory = tempfile::tempdir().expect("temporary directory");
    let part_path = directory.path().join("parallel.part");
    let split = 9_u64;
    let total = PAYLOAD.len() as u64;
    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

    let result = engine
        .download(
            DownloadRequest {
                url: format!("http://{address}/file")
                    .parse()
                    .expect("fixture URL"),
                part_path: part_path.clone(),
                total_bytes: Some(total),
                etag: Some("\"fixture\"".to_owned()),
                last_modified: None,
                use_ranges: true,
                chunks: vec![
                    ChunkSpec {
                        id: ChunkId::new(),
                        start: 0,
                        end: Some(split),
                        committed: 0,
                    },
                    ChunkSpec {
                        id: ChunkId::new(),
                        start: split,
                        end: Some(total),
                        committed: split,
                    },
                ],
                headers: Vec::new(),
                method: rd_core::ReplayMethod::Get,
                body: None,
                approved_origins: Arc::new(Vec::new()),
                captured_user_agent: None,
                transform: None,
            },
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await
        .expect("parallel download");

    assert_eq!(result, super::DownloadOutcome::Complete);
    assert_eq!(
        tokio::fs::read(part_path).await.expect("part file"),
        PAYLOAD
    );
}

#[test]
fn retry_after_and_auth_statuses_keep_their_taxonomy() {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::RETRY_AFTER, "17".parse().expect("header"));
    let HttpDownloadError::Failure(rate_limited) =
        status_failure(StatusCode::TOO_MANY_REQUESTS, &headers)
    else {
        panic!("expected classified failure");
    };
    assert_eq!(
        rate_limited.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(17)
        }
    );

    let HttpDownloadError::Failure(auth) =
        status_failure(StatusCode::UNAUTHORIZED, &header::HeaderMap::new())
    else {
        panic!("expected classified failure");
    };
    assert_eq!(auth.category, FailureKind::AuthRequired);
}

/// TR-01: a `Retry-After` of years is read as the ceiling, not taken at its word.
#[test]
fn a_huge_retry_after_is_capped_where_it_is_read() {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::RETRY_AFTER,
        u64::MAX.to_string().parse().expect("header"),
    );
    let HttpDownloadError::Failure(failure) =
        status_failure(StatusCode::SERVICE_UNAVAILABLE, &headers)
    else {
        panic!("expected classified failure");
    };
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(rd_core::MAX_RETRY_AFTER_SECONDS)
        }
    );
}

#[test]
fn status_failure_carries_a_translatable_code() {
    let HttpDownloadError::Failure(failure) =
        status_failure(StatusCode::NOT_FOUND, &header::HeaderMap::new())
    else {
        panic!("expected classified failure");
    };
    assert_eq!(failure.code.as_deref(), Some("download.http_status"));
    assert_eq!(
        failure.params.get("status").map(String::as_str),
        Some("404 Not Found")
    );
    assert_eq!(failure.message, "HTTP 404 Not Found");
}

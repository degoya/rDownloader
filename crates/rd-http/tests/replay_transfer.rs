//! Fixture tests for replaying an authenticated request: a POST reaches the origin with its
//! exact body, and a redirect outside the approved origins is refused before any byte — or
//! any header — reaches the foreign host.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rd_core::{ChunkId, ReplayMethod};
use rd_http::{
    CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
    ReplayPayload, ReplayScope,
};
use rd_limits::ScopedLimiter;
use tokio_util::sync::CancellationToken;

struct NoopCheckpoint;

#[async_trait::async_trait]
impl CheckpointSink for NoopCheckpoint {
    async fn commit(&self, _chunk_id: ChunkId, _committed_offset: u64) -> anyhow::Result<()> {
        Ok(())
    }
}

/// What the origin observed, so a test can prove a request never arrived.
#[derive(Default)]
struct Observed {
    hits: AtomicUsize,
    saw_content_type: AtomicBool,
    body: std::sync::Mutex<Vec<u8>>,
    user_agent: std::sync::Mutex<Option<String>>,
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    address
}

/// An origin that serves the payload only for a POST carrying the exact expected body.
async fn post_origin(expected: &'static [u8], observed: Arc<Observed>) -> SocketAddr {
    async fn handler(
        State((expected, observed)): State<(&'static [u8], Arc<Observed>)>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        observed.hits.fetch_add(1, Ordering::SeqCst);
        *observed.body.lock().expect("lock") = body.to_vec();
        *observed.user_agent.lock().expect("lock") = headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if headers
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value == "application/x-www-form-urlencoded")
        {
            observed.saw_content_type.store(true, Ordering::SeqCst);
        }
        if body.as_ref() == expected {
            (StatusCode::OK, "PAYLOAD!").into_response()
        } else {
            (StatusCode::BAD_REQUEST, "wrong body").into_response()
        }
    }
    serve(
        Router::new()
            .route("/dl", post(handler))
            .with_state((expected, observed)),
    )
    .await
}

#[tokio::test]
async fn a_post_replay_sends_the_exact_body_and_captured_agent() {
    let observed = Arc::new(Observed::default());
    let address = post_origin(b"id=42&token=s3cr3t", Arc::clone(&observed)).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("payload.part");

    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());
    let outcome = engine
        .download(
            DownloadRequest {
                method: ReplayMethod::Post,
                body: Some(ReplayPayload {
                    content_type: "application/x-www-form-urlencoded".to_owned(),
                    bytes: Bytes::from_static(b"id=42&token=s3cr3t"),
                }),
                captured_user_agent: Some("Mozilla/5.0 (captured)".to_owned()),
                ..DownloadRequest::get(
                    format!("http://{address}/dl").parse().expect("url"),
                    part_path.clone(),
                    Some(8),
                    vec![ChunkSpec {
                        id: ChunkId::new(),
                        start: 0,
                        end: None,
                        committed: 0,
                    }],
                )
            },
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await
        .expect("download");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(
        tokio::fs::read(&part_path).await.expect("read"),
        b"PAYLOAD!"
    );
    assert_eq!(
        *observed.body.lock().expect("lock"),
        b"id=42&token=s3cr3t".to_vec()
    );
    assert!(observed.saw_content_type.load(Ordering::SeqCst));
    // The captured agent has to reach the origin, or a signed URL check may reject the replay.
    assert_eq!(
        observed.user_agent.lock().expect("lock").as_deref(),
        Some("Mozilla/5.0 (captured)")
    );
}

#[tokio::test]
async fn a_post_is_never_split_into_parallel_chunks() {
    // Issuing a POST N times in parallel could trigger the origin's side effect N times.
    let observed = Arc::new(Observed::default());
    let address = post_origin(b"x=1", Arc::clone(&observed)).await;
    let directory = tempfile::tempdir().expect("tempdir");

    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());
    let result = engine
        .download(
            DownloadRequest {
                method: ReplayMethod::Post,
                use_ranges: true,
                ..DownloadRequest::get(
                    format!("http://{address}/dl").parse().expect("url"),
                    directory.path().join("p.part"),
                    Some(8),
                    vec![
                        ChunkSpec {
                            id: ChunkId::new(),
                            start: 0,
                            end: Some(4),
                            committed: 0,
                        },
                        ChunkSpec {
                            id: ChunkId::new(),
                            start: 4,
                            end: Some(8),
                            committed: 0,
                        },
                    ],
                )
            },
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await;

    assert!(matches!(result, Err(HttpDownloadError::RangeIgnored)));
    assert_eq!(
        observed.hits.load(Ordering::SeqCst),
        0,
        "the origin must not be contacted at all"
    );
}

/// Server A redirects to server B; B records whether it was ever contacted.
async fn redirecting_pair(observed: Arc<Observed>) -> (SocketAddr, SocketAddr) {
    async fn foreign(State(observed): State<Arc<Observed>>, headers: HeaderMap) -> Response {
        observed.hits.fetch_add(1, Ordering::SeqCst);
        if headers.contains_key(header::COOKIE) || headers.contains_key(header::AUTHORIZATION) {
            observed.saw_content_type.store(true, Ordering::SeqCst);
        }
        (StatusCode::OK, "FOREIGN!").into_response()
    }
    let foreign_address = serve(
        Router::new()
            .route("/f", get(foreign))
            .with_state(Arc::clone(&observed)),
    )
    .await;

    let target = format!("http://{foreign_address}/f");
    let redirect = move || {
        let target = target.clone();
        async move {
            Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, target)
                .body(axum::body::Body::empty())
                .expect("response")
        }
    };
    let origin_address = serve(Router::new().route("/dl", get(redirect))).await;
    (origin_address, foreign_address)
}

#[tokio::test]
async fn a_redirect_outside_the_approved_origins_is_refused_and_never_contacted() {
    let observed = Arc::new(Observed::default());
    let (origin, foreign) = redirecting_pair(Arc::clone(&observed)).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("p.part");

    // Only the first origin is approved; the redirect target deliberately is not.
    let scope = ReplayScope::new(vec![format!("http://{origin}")]).expect("scope");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    let engine = DownloadEngine::new(client, ScopedLimiter::unlimited());
    let result = engine
        .download(
            DownloadRequest {
                approved_origins: Arc::new(scope.approved_origins.clone()),
                ..DownloadRequest::get(
                    format!("http://{origin}/dl").parse().expect("url"),
                    part_path.clone(),
                    None,
                    vec![ChunkSpec {
                        id: ChunkId::new(),
                        start: 0,
                        end: None,
                        committed: 0,
                    }],
                )
            },
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await;

    match result {
        Err(HttpDownloadError::Failure(failure)) => {
            assert_eq!(
                failure.code.as_deref(),
                Some("download.redirect_not_allowed")
            );
            // The refused target is reported redacted, never raw.
            assert!(failure.params.contains_key("target"), "{failure:?}");
        }
        other => panic!("expected a refused redirect, got {other:?}"),
    }
    assert_eq!(
        observed.hits.load(Ordering::SeqCst),
        0,
        "the unapproved origin must never be contacted"
    );
    assert_eq!(
        tokio::fs::read(&part_path).await.unwrap_or_default().len(),
        0,
        "no byte may be written"
    );
    let _ = foreign;
}

#[tokio::test]
async fn a_redirect_into_an_approved_origin_is_followed() {
    let observed = Arc::new(Observed::default());
    let (origin, foreign) = redirecting_pair(Arc::clone(&observed)).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let part_path = directory.path().join("p.part");

    let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());
    let outcome = engine
        .download(
            DownloadRequest {
                approved_origins: Arc::new(vec![
                    format!("http://{origin}"),
                    format!("http://{foreign}"),
                ]),
                ..DownloadRequest::get(
                    format!("http://{origin}/dl").parse().expect("url"),
                    part_path.clone(),
                    Some(8),
                    vec![ChunkSpec {
                        id: ChunkId::new(),
                        start: 0,
                        end: None,
                        committed: 0,
                    }],
                )
            },
            Arc::new(NoopCheckpoint),
            CancellationToken::new(),
        )
        .await
        .expect("download");

    assert_eq!(outcome, DownloadOutcome::Complete);
    assert_eq!(
        tokio::fs::read(&part_path).await.expect("read"),
        b"FOREIGN!"
    );
    assert_eq!(observed.hits.load(Ordering::SeqCst), 1);
}

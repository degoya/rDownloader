//! RD-1490-13: a link whose host does not answer ends its check within its deadline and holds
//! no other link.
//!
//! In the live check of 2026-11-26 a made-up `mega.nz/file/…` link stayed "checking" for more
//! than three minutes while the links behind it waited. mega.nz never answers a `HEAD`; the
//! probe gave up after 15 s, and the reclassification then asked the same host again with no
//! deadline at all, on the one task every check job ran on. The hosts here behave like that one,
//! like one that never sends a byte and like one that sends its headers and stalls the body. The
//! links beside them in the batch, and a batch added after it, are checked while they still wait.

use std::time::{Duration, Instant};

use axum::{
    Router,
    http::{Method, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::common;

/// Longer than every check here may take, for a runner under load.
const ENDS_WITHIN: Duration = Duration::from_secs(90);

/// A host that takes every connection and answers `answer` to the first request on it — or
/// nothing at all — then holds the socket until the test ends.
async fn raw_host(answer: fn(&[u8]) -> Option<&'static [u8]>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = vec![0_u8; 4096];
            let read = socket.read(&mut request).await.unwrap_or(0);
            if let Some(bytes) = answer(&request[..read])
                && socket.write_all(bytes).await.is_err()
            {
                continue;
            }
            held.push(socket);
        }
    });
    address
}

/// Never sends a byte.
fn answers_nothing(_request: &[u8]) -> Option<&'static [u8]> {
    None
}

/// Refuses the `HEAD`, then sends the GET's headers and never a byte of its body.
fn stalls_after_headers(request: &[u8]) -> Option<&'static [u8]> {
    let answer: &'static [u8] = if request.starts_with(b"HEAD") {
        b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\
          Connection: close\r\n\r\n"
    } else {
        b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
          Transfer-Encoding: chunked\r\n\r\n"
    };
    Some(answer)
}

/// Like mega.nz: no answer to a `HEAD`, and its page to a GET, whatever the range.
fn mega_like() -> Router {
    Router::new().fallback(|method: Method| async move {
        if method == Method::HEAD {
            std::future::pending::<()>().await;
        }
        let page = format!("<!DOCTYPE html><html>{}</html>", " ".repeat(2048));
        ([(header::CONTENT_TYPE, "text/html")], page).into_response()
    })
}

fn files() -> Router {
    let file = |name: &'static str| {
        get(move || async move {
            (
                [(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                )],
                "payload",
            )
        })
    };
    Router::new()
        .route("/beside.bin", file("beside.bin"))
        .route("/later.bin", file("later.bin"))
}

async fn add_batch(router: &Router, text: String) {
    let (status, body) = common::post_json(
        router,
        "/api/v1/collector/batches",
        // The person's own paste: the fixtures serve from this machine (RD-1190-18).
        json!({ "text": text, "source": "manual" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn candidate(database: &rd_db::Database, url: &str) -> rd_core::LinkCandidate {
    let url: url::Url = url.parse().expect("URL");
    database
        .list_candidates()
        .await
        .expect("candidates")
        .into_iter()
        .find(|candidate| candidate.url == url)
        .expect("the link was taken in")
}

/// Waits until the link at `url` has a recorded check.
async fn checked(
    database: &rd_db::Database,
    url: &str,
    within: Duration,
) -> rd_core::LinkCandidate {
    common::eventually(
        within,
        &format!("{url} was never checked"),
        move || async move {
            let found = candidate(database, url).await;
            (found.checked_at.is_some() && found.state != rd_core::LinkCandidateState::Checking)
                .then_some(found)
        },
    )
    .await
}

#[tokio::test]
async fn a_host_that_does_not_answer_holds_no_other_link() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let database = &harness.database;
    let silent = format!("http://{}/file/SilentHost", raw_host(answers_nothing).await);
    let stalling = format!(
        "http://{}/download?id=1",
        raw_host(stalls_after_headers).await
    );
    let mega_server = rd_testkit::serve(mega_like()).await;
    let mega = format!("http://{}/file/AbCdEfGh", mega_server.addr());
    let file_server = rd_testkit::serve(files()).await;
    let beside = format!("http://{}/beside.bin", file_server.addr());
    let later = format!("http://{}/later.bin", file_server.addr());

    let started = Instant::now();
    add_batch(
        &harness.router,
        [
            silent.as_str(),
            stalling.as_str(),
            mega.as_str(),
            beside.as_str(),
        ]
        .join("\n"),
    )
    .await;
    // The silent host's probe cannot end before its 15 s: a link checked while it still waits
    // was not held by it.
    let found = checked(database, &beside, common::WAIT).await;
    assert_eq!(
        found.state,
        rd_core::LinkCandidateState::Online,
        "{found:?}"
    );
    assert_eq!(
        candidate(database, &silent).await.state,
        rd_core::LinkCandidateState::Checking,
        "the link beside it was recorded first"
    );

    add_batch(&harness.router, later.clone()).await;
    let found = checked(database, &later, common::WAIT).await;
    assert_eq!(
        found.state,
        rd_core::LinkCandidateState::Online,
        "{found:?}"
    );
    assert_eq!(
        candidate(database, &silent).await.state,
        rd_core::LinkCandidateState::Checking,
        "a batch added later is not held by the first one"
    );

    // Each slow host ends within its deadline: inconclusive, a file, a page.
    let found = checked(database, &silent, ENDS_WITHIN).await;
    assert_eq!(
        found.error_code.as_deref(),
        Some("collector.check_inconclusive"),
        "{found:?}"
    );
    let found = checked(database, &stalling, ENDS_WITHIN).await;
    assert_eq!(
        found.state,
        rd_core::LinkCandidateState::Online,
        "{found:?}"
    );
    let found = checked(database, &mega, ENDS_WITHIN).await;
    assert_eq!(
        found.state,
        rd_core::LinkCandidateState::Unresolvable,
        "the GET after the unanswered HEAD found the page: {found:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
}

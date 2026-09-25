//! `POST /api/v1/capture/file` (RD-130-16): a file only the browser could load, handed over as
//! its bytes or as its address with that host's cookies — and never fetched a second time.

mod common;

use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use base64::Engine;
use common::{CAPTURE_BEARER, post_with_bearer, send, test_harness};
use serde_json::json;

const NZB: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="tester" subject="release.bin">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="42" number="1">release@example</segment></segments>
  </file>
</nzb>"#;

const ROUTE: &str = "/api/v1/capture/file";

/// A second, different NZB: an import is keyed by its hash, so the same bytes twice are one import.
fn other_nzb() -> String {
    NZB.replace("release@example", "other@example")
}

fn zip_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in members {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("member");
        writer.write_all(content).expect("content");
    }
    writer.finish().expect("finish").into_inner()
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A multipart upload of one file, as the Firefox extension sends the bytes it copied.
async fn upload(
    router: &Router,
    file_name: &str,
    content: &[u8],
) -> (StatusCode, serde_json::Value) {
    let boundary = "rdcaptureboundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let request = Request::post(ROUTE)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("request");
    send(router, request).await
}

/// What one of the local "indexers" below saw: how often it was asked, and with which cookies.
#[derive(Clone, Default)]
struct Seen {
    hits: Arc<AtomicUsize>,
    cookies: Arc<Mutex<Vec<Option<String>>>>,
}

impl Seen {
    fn record(&self, headers: &HeaderMap) -> Option<String> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        let cookie = headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        self.cookies.lock().expect("lock").push(cookie.clone());
        cookie
    }

    fn cookies(&self) -> Vec<Option<String>> {
        self.cookies.lock().expect("lock").clone()
    }
}

async fn serve(app: Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

/// An NNTmux-like cart: the NZB for a signed-in session, `400 Missing parameter` for anybody else.
async fn indexer(seen: Seen) -> std::net::SocketAddr {
    let app = Router::new().route(
        "/getnzb/abc",
        axum::routing::get(move |headers: HeaderMap| {
            let seen = seen.clone();
            async move {
                match seen.record(&headers).as_deref() {
                    Some("uid=7; sess=s3cr3t") => (
                        StatusCode::OK,
                        [(
                            header::CONTENT_DISPOSITION,
                            "attachment; filename=\"Cart.Release.nzb\"",
                        )],
                        NZB,
                    ),
                    _ => (
                        StatusCode::BAD_REQUEST,
                        [(header::CONTENT_DISPOSITION, "inline")],
                        "Missing parameter",
                    ),
                }
            }
        }),
    );
    serve(app).await
}

/// The session as the extension sends it: the Netscape rows `capture/cookies` takes too, for the
/// local "indexer"'s host. The format carries no port; which origin they go to is the service's
/// rule.
fn session_cookies() -> serde_json::Value {
    json!(
        "# Netscape HTTP Cookie File\n\
         127.0.0.1\tFALSE\t/\tFALSE\t0\tuid\t7\n\
         #HttpOnly_127.0.0.1\tFALSE\t/getnzb\tFALSE\t0\tsess\ts3cr3t\n"
    )
}

#[tokio::test]
async fn the_bytes_of_an_nzb_become_an_import_without_a_fetch() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = upload(&harness.router, "Release.nzb", NZB.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "nzb");
    assert_eq!(body["nzb_imports"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["nzb_imports"][0]["name"], "Release.nzb");
    assert!(body["torrent"].is_null());
}

#[tokio::test]
async fn a_zip_of_nzbs_becomes_one_import_per_nzb() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let archive = zip_of(&[
        ("First.nzb", NZB.as_bytes()),
        ("Second.nzb", other_nzb().as_bytes()),
        ("index.txt", b"not an nzb"),
    ]);
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "content": base64(&archive), "file_name": "cart.zip" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "nzb_zip");
    let names: Vec<&str> = body["nzb_imports"]
        .as_array()
        .expect("imports")
        .iter()
        .filter_map(|import| import["name"].as_str())
        .collect();
    assert_eq!(names, ["First.nzb", "Second.nzb"]);
    assert_eq!(
        harness
            .database
            .list_nzb_imports()
            .await
            .expect("list")
            .len(),
        2
    );
}

/// Half a cart would leave the person guessing which half is missing.
#[tokio::test]
async fn a_zip_with_one_broken_nzb_imports_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let archive = zip_of(&[("Good.nzb", NZB.as_bytes()), (
            "Broken.nzb",
            br#"<nzb><file subject="x"><segments><segment number="one" bytes="1">id</segment></segments></file></nzb>"#,
        )]);
    let (status, body) = upload(&harness.router, "cart.zip", &archive).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "nzb.parse_failed");
    assert!(
        harness
            .database
            .list_nzb_imports()
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn the_bytes_of_a_torrent_reach_the_linkgrabber() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let mut torrent =
        b"d8:announce31:http://tracker.example/announce4:infod6:lengthi32e4:name7:release12:piece lengthi16384e6:pieces20:"
            .to_vec();
    torrent.extend_from_slice(&[0_u8; 20]);
    torrent.extend_from_slice(b"ee");
    let (status, body) = upload(&harness.router, "release.torrent", &torrent).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "torrent");
    assert_eq!(
        body["torrent"]["candidates"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(body["nzb_imports"].as_array().map(Vec::len), Some(0));
}

/// An expired session answers with a login page; that stays with the browser.
#[tokio::test]
async fn what_is_not_an_nzb_a_torrent_or_a_zip_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = upload(
        &harness.router,
        "Release.nzb",
        b"<!doctype html><title>Login</title>",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "capture.file_unsupported");
}

#[tokio::test]
async fn bytes_and_an_address_together_or_neither_are_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for request in [
        json!({}),
        json!({ "content": base64(NZB.as_bytes()), "url": "https://indexer.test/getnzb/abc" }),
        // Cookies belong to a fetch; next to bytes they would only be carried around.
        json!({ "content": base64(NZB.as_bytes()), "cookies": session_cookies() }),
    ] {
        let (status, body) =
            post_with_bearer(&harness.router, ROUTE, CAPTURE_BEARER, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "capture.file_source_invalid");
    }
}

#[tokio::test]
async fn the_route_needs_the_capture_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, _) = post_with_bearer(
        &harness.router,
        ROUTE,
        "not-a-token",
        json!({ "content": base64(NZB.as_bytes()) }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The acceptance criterion in one test: the session's cookies reach the cart, the cart is
/// asked exactly once, and what it answered is imported.
#[tokio::test]
async fn an_address_with_its_cookies_is_fetched_exactly_once() {
    let seen = Seen::default();
    let address = indexer(seen.clone()).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({
            "url": format!("http://{address}/getnzb/abc"),
            "cookies": session_cookies(),
            "referrer": format!("http://{address}/cart"),
            "user_agent": "Mozilla/5.0 (X11; Linux x86_64; rv:156.0) Gecko/20100101 Firefox/156.0"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "nzb");
    assert_eq!(body["nzb_imports"][0]["name"], "Cart.Release.nzb");
    assert_eq!(
        seen.hits.load(Ordering::SeqCst),
        1,
        "the cart counts every fetch"
    );
}

/// Without the session the cart answers an error, and that error is what the person hears —
/// not a file imported from an error page.
#[tokio::test]
async fn a_refused_fetch_is_a_bad_gateway_and_imports_nothing() {
    let seen = Seen::default();
    let address = indexer(seen.clone()).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "url": format!("http://{address}/getnzb/abc") }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "capture.fetch_failed");
    assert_eq!(body["params"]["status"], "400");
    assert_eq!(seen.hits.load(Ordering::SeqCst), 1);
    assert_eq!(seen.cookies(), [None]);
    assert!(
        harness
            .database
            .list_nzb_imports()
            .await
            .expect("list")
            .is_empty()
    );
}

/// A redirect to another origin is followed, but the cookies stay behind: they were handed
/// over for one host, and a port is part of what makes a host.
#[tokio::test]
async fn cookies_never_follow_a_redirect_to_another_origin() {
    let elsewhere = Seen::default();
    let target = {
        let seen = elsewhere.clone();
        serve(Router::new().route(
            "/file.nzb",
            axum::routing::get(move |headers: HeaderMap| {
                let seen = seen.clone();
                async move {
                    seen.record(&headers);
                    NZB
                }
            }),
        ))
        .await
    };
    let origin = Seen::default();
    let start = {
        let seen = origin.clone();
        serve(Router::new().route(
            "/getnzb/abc",
            axum::routing::get(move |headers: HeaderMap| {
                let seen = seen.clone();
                async move {
                    seen.record(&headers);
                    axum::response::Redirect::temporary(&format!("http://{target}/file.nzb"))
                }
            }),
        ))
        .await
    };
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "url": format!("http://{start}/getnzb/abc"), "cookies": session_cookies() }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(origin.cookies(), [Some("uid=7; sess=s3cr3t".to_owned())]);
    assert_eq!(elsewhere.cookies(), [None]);
    assert_eq!(body["nzb_imports"][0]["name"], "file.nzb");
}

#[tokio::test]
async fn a_cookie_that_could_break_the_header_is_refused_before_anything_is_sent() {
    let seen = Seen::default();
    let address = indexer(seen.clone()).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({
            "url": format!("http://{address}/getnzb/abc"),
            "cookies": "uid=7\r\nX-Injected: 1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "capture.cookies_invalid");
    assert!(
        !body.to_string().contains("X-Injected"),
        "the value is never echoed"
    );
    assert_eq!(seen.hits.load(Ordering::SeqCst), 0);
}

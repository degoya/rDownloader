use std::time::Duration;

use axum::http::{StatusCode, header};
use url::Url;

use super::{CnlState, MAX_ADDCRYPTED_BODY_BYTES, NO_LINK_DETAIL, Pace, code, router};
use crate::client::CaptureClient;

/// A live listener on an ephemeral port, so the routing rules are exercised the way a
/// browser would meet them rather than through a hand-built request.
///
/// The client points at a port nothing listens on: every test here is about a request that
/// is refused before anything is handed over, and a hand-over that did happen would fail
/// loudly rather than reach a real service.
async fn spawn() -> rd_testkit::TestServer {
    spawn_paced(Pace::new(super::pace::MAX_HAND_OVERS, super::pace::WINDOW)).await
}

/// [`spawn`] with a pace of the test's own.
async fn spawn_paced(pace: Pace) -> rd_testkit::TestServer {
    let state = CnlState {
        client: CaptureClient::new(
            Url::parse("http://127.0.0.1:9/").expect("valid URL"),
            "x".repeat(32),
        )
        .expect("build a capture client"),
        pace: std::sync::Arc::new(pace),
    };
    rd_testkit::serve(router(state)).await
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build a client")
}

/// The agent's own NZB route is gone (RD-1200-03): `open` hands an NZB to the service itself,
/// with the pairing token. A program of another account that finds the port can push no NZB
/// through the agent's credential, with a page's headers or without.
#[tokio::test]
async fn the_port_takes_no_nzb_for_the_agent_to_hand_over() {
    let server = spawn().await;
    let address = server.addr();
    let endpoint = format!("http://{address}/rdownloader/nzb");
    for origin in [None, Some("https://evil.test")] {
        let mut request = client().post(&endpoint).body("<nzb/>");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let response = request.send().await.expect("the listener answers");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{origin:?}");
    }
}

/// The JDownloader-compatible routes keep their wildcard, as the documented exception.
#[tokio::test]
async fn the_flash_routes_keep_their_documented_wildcard() {
    let server = spawn().await;
    let address = server.addr();
    let response = client()
        .get(format!("http://{address}/flash"))
        .header(header::ORIGIN, "https://hoster.test")
        .send()
        .await
        .expect("the listener answers");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("*")
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-private-network")
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
}

/// A page that posts with `fetch()` and a header of its own is asked about first: hide.cx
/// sends `X-Referer`, and a preflight that does not name it makes the browser drop the post.
#[tokio::test]
async fn the_preflight_allows_the_headers_hoster_pages_send() {
    let server = spawn().await;
    let address = server.addr();
    let response = client()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{address}/flash/add"),
        )
        .header(header::ORIGIN, "https://hide.cx")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "content-type,x-referer",
        )
        .send()
        .await
        .expect("the listener answers");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let allowed: Vec<String> = response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .collect();
    for wanted in ["content-type", "x-referer"] {
        assert!(
            allowed.iter().any(|name| name == wanted),
            "{wanted} must be allowed: {allowed:?}"
        );
    }
}

/// The body of a refusal is a code. Neither the decryption's own account of itself nor
/// anything the service said may be readable by the page that made the call.
#[tokio::test]
async fn a_refusal_answers_with_a_code_and_no_prose() {
    let server = spawn().await;
    let address = server.addr();
    let response = client()
        .post(format!("http://{address}/flash/addcrypted2"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(
            "crypted=bm90IHBhZGRlZCBjb3JyZWN0bHkh\
             &jk=function%20f()%7B%20return%20%27abababababababababababababababab%27%3B%20%7D",
        )
        .send()
        .await
        .expect("the listener answers");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("a body");
    assert_eq!(body, code::INVALID_PAYLOAD);
    assert!(
        !body.contains("invalid CNL padding"),
        "the padding oracle must not be readable: {body}"
    );

    // The same for a hand-over the service refused: the agent's account of it stays in the
    // log. Nothing listens on the service port here, so this is the unreachable case.
    let response = client()
        .post(format!("http://{address}/flash/add"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body("urls=https%3A%2F%2Fexample.com%2Ffile.bin")
        .send()
        .await
        .expect("the listener answers");
    let body = response.text().await.expect("a body");
    assert_eq!(body, code::SERVICE_UNAVAILABLE);
    assert!(
        !body.contains("127.0.0.1"),
        "nothing about the service may leak into the answer: {body}"
    );
}

/// `crossdomain.xml` is gone, and `OPTIONS` no longer says yes to paths that do not exist.
#[tokio::test]
async fn only_paths_that_exist_answer_at_all() {
    let server = spawn().await;
    let address = server.addr();
    for path in ["/crossdomain.xml", "/nothing/here"] {
        for method in [reqwest::Method::GET, reqwest::Method::OPTIONS] {
            let response = client()
                .request(method.clone(), format!("http://{address}{path}"))
                .send()
                .await
                .expect("the listener answers");
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} must not be answered"
            );
        }
    }
    // A path that does exist still answers its preflight.
    let response = client()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{address}/flash/add"),
        )
        .send()
        .await
        .expect("the listener answers");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// The limit that matters has to be reached before the body is decoded, not after. A 413
/// rather than the handler's 400 is what tells the two apart.
///
/// Only the declared length is sent, over a raw socket. The listener answers on the header
/// and closes without reading the body, and a client still writing 20 MiB into that socket
/// sees a reset instead of the answer on Windows (os error 10053, CI on 2026-09-25) — the
/// refusal the test is about happened, the test just could not read it.
#[tokio::test]
async fn an_oversized_addcrypted_body_is_refused_before_it_is_decoded() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let server = spawn().await;
    let address = server.addr();
    let mut socket = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect to the listener");
    let request = format!(
        "POST /flash/addcrypted2 HTTP/1.1\r\nHost: {address}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\ncrypted=",
        MAX_ADDCRYPTED_BODY_BYTES + 16
    );
    socket
        .write_all(request.as_bytes())
        .await
        .expect("send the request head");
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut answer)).await;
    let status_line = String::from_utf8_lossy(&answer)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        status_line.starts_with("HTTP/1.1 413"),
        "the body limit has to fire before the form extractor runs, got {status_line:?}"
    );
}

/// The refusal used to name HTTP(S) only, while the collector had long taken more.
#[test]
fn the_link_free_refusal_names_the_schemes_that_are_really_accepted() {
    for sample in [
        "http://example.com/a",
        "https://example.com/a",
        "ftp://example.com/a",
        "ftps://example.com/a",
        "sftp://example.com/a",
        "webdav://example.com/a",
        "webdavs://example.com/a",
        "dav://example.com/a",
        "davs://example.com/a",
        "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01",
    ] {
        let scheme = sample.split([':', '/']).next().expect("a scheme");
        assert!(
            NO_LINK_DETAIL.contains(scheme),
            "the refusal has to name {scheme}, which the collector accepts"
        );
        assert!(
            !rd_collector::extract_urls(sample).is_empty(),
            "the refusal names {scheme}, so the collector has to take it: {sample}"
        );
    }
}

/// RD-1190-22: a page could send hand-overs as fast as the loopback carries them, each a
/// LinkGrabber batch with its online checks. Past the allowance the answer is `429` and a code,
/// and nothing is handed over.
#[tokio::test]
async fn hand_overs_past_the_allowance_are_refused() {
    let server = spawn_paced(Pace::new(1, Duration::from_secs(60))).await;
    let address = server.addr();
    let mut codes = Vec::new();
    for _ in 0..2 {
        let response = client()
            .post(format!("http://{address}/flash/add"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body("urls=https%3A%2F%2Fexample.com%2Ffile.bin")
            .send()
            .await
            .expect("the listener answers");
        codes.push((response.status(), response.text().await.expect("a body")));
    }
    // The first one was admitted and failed only at the unreachable service.
    assert_eq!(codes[0].1, code::SERVICE_UNAVAILABLE);
    assert_eq!(
        codes[1],
        (StatusCode::TOO_MANY_REQUESTS, code::RATE_LIMITED.to_owned())
    );
}

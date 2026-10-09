use std::{net::SocketAddr, time::Duration};

use axum::http::{StatusCode, header};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::jk::{JK_SLOTS, MAX_CONCURRENT_JK, extract_static_key, resolve_key, resolve_key_within};
use super::{CnlState, MAX_ADDCRYPTED_BODY_BYTES, NO_LINK_DETAIL, Pace, code, router};
use crate::client::CaptureClient;

/// For tests that expect an evaluation to finish. The production budget of 250 ms was
/// overrun by Boa's first evaluation on a loaded GitHub runner (2026-09-25), which turned a
/// test of the result into a test of the machine; the expiry has its own tests.
async fn resolve_patiently(source: &str) -> anyhow::Result<[u8; 16]> {
    resolve_key_within(source, Duration::from_secs(10)).await
}

/// A live listener on an ephemeral port, so the routing rules are exercised the way a
/// browser would meet them rather than through a hand-built request.
///
/// The client points at a port nothing listens on: every test here is about a request that
/// is refused before anything is handed over, and a hand-over that did happen would fail
/// loudly rather than reach a real service.
async fn spawn() -> (SocketAddr, CancellationToken) {
    spawn_paced(Pace::new(super::pace::MAX_HAND_OVERS, super::pace::WINDOW)).await
}

/// [`spawn`] with a pace of the test's own.
async fn spawn_paced(pace: Pace) -> (SocketAddr, CancellationToken) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let address = listener.local_addr().expect("the bound address");
    let cancellation = CancellationToken::new();
    let state = CnlState {
        client: CaptureClient::new(
            Url::parse("http://127.0.0.1:9/").expect("valid URL"),
            "x".repeat(32),
        )
        .expect("build a capture client"),
        pace: std::sync::Arc::new(pace),
    };
    let shutdown = cancellation.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state))
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await;
    });
    (address, cancellation)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build a client")
}

#[test]
fn accepts_literal_or_javascript_wrapped_static_keys() {
    let expected = [0xab_u8; 16];
    assert_eq!(
        extract_static_key("function f(){ return 'abababababababababababababababab'; }")
            .expect("one unambiguous literal"),
        Some(expected)
    );
    // The `key` form: the field is the key itself.
    assert_eq!(
        extract_static_key("  abababababababababababababababab  ").expect("a bare key"),
        Some(expected)
    );
}

#[tokio::test]
async fn evaluates_non_literal_key_in_boa_without_host_apis() {
    let script =
        "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
    assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
}

#[tokio::test]
async fn rejects_unbounded_javascript_loops() {
    let script = "function getKey(){ while(true){} }";
    assert!(resolve_key(script).await.is_err());
}

/// The agent's own NZB route is gone (RD-1200-03): `open` hands an NZB to the service itself,
/// with the pairing token. A program of another account that finds the port can push no NZB
/// through the agent's credential, with a page's headers or without.
#[tokio::test]
async fn the_port_takes_no_nzb_for_the_agent_to_hand_over() {
    let (address, cancellation) = spawn().await;
    let endpoint = format!("http://{address}/rdownloader/nzb");
    for origin in [None, Some("https://evil.test")] {
        let mut request = client().post(&endpoint).body("<nzb/>");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let response = request.send().await.expect("the listener answers");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{origin:?}");
    }
    cancellation.cancel();
}

/// The JDownloader-compatible routes keep their wildcard, as the documented exception.
#[tokio::test]
async fn the_flash_routes_keep_their_documented_wildcard() {
    let (address, cancellation) = spawn().await;
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
    cancellation.cancel();
}

/// A page that posts with `fetch()` and a header of its own is asked about first: hide.cx
/// sends `X-Referer`, and a preflight that does not name it makes the browser drop the post.
#[tokio::test]
async fn the_preflight_allows_the_headers_hoster_pages_send() {
    let (address, cancellation) = spawn().await;
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
    cancellation.cancel();
}

/// The body of a refusal is a code. Neither the decryption's own account of itself nor
/// anything the service said may be readable by the page that made the call.
#[tokio::test]
async fn a_refusal_answers_with_a_code_and_no_prose() {
    let (address, cancellation) = spawn().await;
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
    cancellation.cancel();
}

/// `crossdomain.xml` is gone, and `OPTIONS` no longer says yes to paths that do not exist.
#[tokio::test]
async fn only_paths_that_exist_answer_at_all() {
    let (address, cancellation) = spawn().await;
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
    cancellation.cancel();
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

    let (address, cancellation) = spawn().await;
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
    cancellation.cancel();
}

/// The timeout used to end only the waiting. What it has to end now is the claim on the
/// runtime: the agent is still able to evaluate the next script right afterwards.
#[tokio::test]
async fn an_expired_jk_evaluation_leaves_the_agent_working() {
    let spinning = "function getKey(){ var n=0; while(true){ n=n+1; } }";
    let expired = resolve_key_within(spinning, Duration::from_millis(1)).await;
    assert!(expired.is_err(), "a script that never returns must not win");

    // The next caller is served, which is the property the blocking pool used to lose.
    let script =
        "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
    assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
}

/// A page must not be able to open one evaluation per request.
#[tokio::test]
async fn concurrent_jk_evaluations_are_capped() {
    let held = JK_SLOTS
        .try_acquire_many(u32::try_from(MAX_CONCURRENT_JK).expect("a small cap"))
        .expect("every slot is free at the start of this test");
    let script =
        "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
    let refused = resolve_key(script).await;
    assert!(
        refused.is_err(),
        "with every slot taken the call has to be refused, not queued"
    );
    drop(held);
    assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
}

/// The old pattern matched anywhere, so an unrelated identifier or the first half of a
/// 64-digit literal silently became the key and decryption then failed with a padding
/// error that pointed at nothing.
#[tokio::test]
async fn an_ambiguous_static_key_is_refused_rather_than_guessed_at() {
    let two_literals = "function getKey(){ var id='0123456789abcdef0123456789abcdef'; \
                        return 'abababababababababababababababab'; }";
    assert!(
        extract_static_key(two_literals).is_err(),
        "two candidates cannot be resolved by taking the first"
    );
    assert!(resolve_key(two_literals).await.is_err());

    let sixty_four = format!("function getKey(){{ return '{}'; }}", "ab".repeat(32));
    assert_eq!(
        extract_static_key(&sixty_four).expect("no literal key, not an error"),
        None,
        "a 64-digit literal is not a 128-bit key and must not be cut in half"
    );
    // It is then evaluated, and the 64-digit result is refused with a reason.
    let error = resolve_patiently(&sixty_four)
        .await
        .expect_err("a 64-digit result is not a key");
    assert!(
        error.to_string().contains("128-bit hexadecimal key"),
        "{error}"
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
    let (address, cancellation) = spawn_paced(Pace::new(1, Duration::from_secs(60))).await;
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
    cancellation.cancel();
}

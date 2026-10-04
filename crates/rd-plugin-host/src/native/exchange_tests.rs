//! One plugin request against a local server: the time limit covers the body (PLUG-03), and
//! the size limit is the invocation's allowance rather than a fixed cap (PLUG-06).

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

use super::{
    DEFAULT_RESPONSE_BYTES, EXCHANGE_TIMEOUT, Limits, MAX_UPLOAD_TIME, SEND_TIMEOUT, exchange,
    response_limit, upload_allowance, with_response_allowance,
};

/// Answers one request with `head`, then `body`, then holds the connection open.
async fn serve_once(head: String, body: Vec<u8>) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(read) => request.extend_from_slice(&buffer[..read]),
            }
        }
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(&body).await;
        // A slow server: the rest of the promised body never comes.
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    format!("http://{address}/listing").parse().expect("url")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client")
}

/// PLUG-03: headers in time and then a body that never finishes is a timeout, not a call that
/// hangs as long as the server likes.
#[tokio::test]
async fn a_body_that_never_finishes_runs_into_the_time_limit() {
    let url = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n".to_owned(),
        vec![b'x'; 10],
    )
    .await;
    let started = std::time::Instant::now();

    let failure = exchange(
        client().get(url.clone()),
        &url,
        false,
        DEFAULT_RESPONSE_BYTES,
        Limits {
            head: SEND_TIMEOUT,
            whole: Duration::from_millis(500),
        },
        &crate::OwnEndpoints::default(),
    )
    .await
    .expect_err("the body never completes");

    assert_eq!(failure.code.as_deref(), Some("plugin.http_timeout"));
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// PLUG-06: a 16 MiB answer — a large MEGA listing — fits a 16 MiB allowance and is refused
/// one byte below it. Before, a fixed 8 MiB refused it whatever the manifest declared.
#[tokio::test]
async fn the_allowance_not_a_fixed_cap_decides_how_large_an_answer_may_be() {
    const SIXTEEN_MIB: usize = 16 * 1024 * 1024;
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {SIXTEEN_MIB}\r\n\r\n");

    let url = serve_once(head.clone(), vec![b'x'; SIXTEEN_MIB]).await;
    let response = exchange(
        client().get(url.clone()),
        &url,
        false,
        SIXTEEN_MIB,
        Limits {
            head: SEND_TIMEOUT,
            whole: Duration::from_secs(30),
        },
        &crate::OwnEndpoints::default(),
    )
    .await
    .expect("16 MiB fits a 16 MiB allowance");
    assert_eq!(response.body.len(), SIXTEEN_MIB);

    let url = serve_once(head, vec![b'x'; SIXTEEN_MIB]).await;
    let failure = exchange(
        client().get(url.clone()),
        &url,
        false,
        SIXTEEN_MIB - 1,
        Limits {
            head: SEND_TIMEOUT,
            whole: Duration::from_secs(30),
        },
        &crate::OwnEndpoints::default(),
    )
    .await
    .expect_err("one byte short");
    assert_eq!(failure.code.as_deref(), Some("plugin.response_too_large"));
}

/// The allowance comes from the invocation, is held to the manifest ceiling, and a request
/// made outside any invocation keeps the old default.
#[tokio::test]
async fn the_limit_is_the_invocations_allowance_under_the_manifest_ceiling() {
    assert_eq!(response_limit(), DEFAULT_RESPONSE_BYTES);
    let sixteen = with_response_allowance(16 * 1024 * 1024, async { response_limit() }).await;
    assert_eq!(sixteen, 16 * 1024 * 1024);
    let ceiling = with_response_allowance(u64::MAX, async { response_limit() }).await;
    assert_eq!(
        u64::try_from(ceiling).expect("fits"),
        crate::manifest::MAX_RESPONSE_BYTES
    );
}

/// RA-HOST-04: sending the body comes on top of both limits. 15 s used to cover the upload as
/// well as the server's answer, so a WebDAV `PUT` failed past ~18 MiB at 10 Mbit/s and a 64 MiB
/// body needed more than 1 MB/s to fit the 60 s.
#[test]
fn a_request_body_buys_time_by_its_size_up_to_a_ceiling() {
    assert_eq!(upload_allowance(0), Duration::ZERO);
    assert_eq!(upload_allowance(1), Duration::from_secs(1));
    // 64 MiB at 64 KiB/s: 1024 s on top of the head and the whole limit.
    let body = 64 * 1024 * 1024;
    assert_eq!(upload_allowance(body), Duration::from_secs(1024));
    assert!(SEND_TIMEOUT + upload_allowance(body) > Duration::from_secs(64 * 8 * 1024 / 10_000));
    assert!(EXCHANGE_TIMEOUT + upload_allowance(body) > SEND_TIMEOUT + upload_allowance(body));
    // Never past the ceiling, whatever the length says.
    assert_eq!(upload_allowance(usize::MAX), MAX_UPLOAD_TIME);
}

//! RD-1490-13: a direct link's probe ends within its deadlines, whatever the host does.

use std::time::{Duration, Instant};

use super::{Deadlines, DirectProbe, probe_direct_within};

const SHORT: Deadlines = Deadlines {
    probe: Duration::from_millis(400),
    read: Duration::from_millis(400),
};

/// A host that takes the connection and never sends a byte; the sockets are held until the
/// test ends.
async fn silent_host() -> (url::Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let url = format!("http://{address}/file/AbCdEfGh")
        .parse()
        .expect("URL");
    (url, task)
}

#[tokio::test]
async fn a_host_that_never_answers_ends_within_the_probe_deadline() {
    let (url, _host) = silent_host().await;
    let started = Instant::now();
    let (result, probed) = probe_direct_within(&reqwest::Client::new(), &[], url, SHORT).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(result.status, rd_core::LinkStatus::Unknown);
    // Not `Unasked`: the reclassification would ask the silent host a second time.
    assert!(matches!(probed, DirectProbe::Failed));
}

/// Refuses the `HEAD` like an indexer proxy, then answers the GET with its headers and stalls
/// the body for good.
async fn stalling_host() -> (url::Url, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = vec![0_u8; 4096];
            let Ok(read) = socket.read(&mut request).await else {
                continue;
            };
            let answer: &[u8] = if request[..read].starts_with(b"HEAD") {
                b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\
                  Connection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                  Transfer-Encoding: chunked\r\n\r\n"
            };
            if socket.write_all(answer).await.is_ok() {
                held.push(socket);
            }
        }
    });
    let url = format!("http://{address}/download?id=1")
        .parse()
        .expect("URL");
    (url, task)
}

#[tokio::test]
async fn an_answer_that_stalls_after_its_headers_is_marked_and_ends() {
    let (url, _host) = stalling_host().await;
    let started = Instant::now();
    let (result, probed) = probe_direct_within(&reqwest::Client::new(), &[], url, SHORT).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        result.status,
        rd_core::LinkStatus::Online,
        "the headers name a file"
    );
    let DirectProbe::Answered(probed) = probed else {
        panic!("the probe answered");
    };
    assert!(
        probed.stalled,
        "the stall is kept, so nothing reads the address again"
    );
    assert!(probed.body.is_none());
}

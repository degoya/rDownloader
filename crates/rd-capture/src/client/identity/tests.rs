//! The token goes only where rDownloader answers (RD-1200-03).

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, atomic::Ordering},
};

use reqwest::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{ForeignListener, Identity, answers_as_rdownloader};
use crate::client::{CaptureClient, Purpose, build};

/// The health answer of the real service (`rd-api/src/handlers.rs::health`).
const SERVICE_HEALTH: &str = r#"{"status":"ok","version":"1.20.0","service":"rDownloader"}"#;

/// A listener answering every health request with `health` and everything else with a 401,
/// keeping the head of every request it was sent.
async fn listener(health: &'static str) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let address = listener.local_addr().expect("the bound address");
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&heads);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                }
            }
            let head = String::from_utf8_lossy(&buffer).into_owned();
            let (status, body) = if head.starts_with("GET /api/v1/health ") {
                ("200 OK", health)
            } else {
                ("401 Unauthorized", r#"{"code":"capture.token_required"}"#)
            };
            seen.lock().expect("record").push(head);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (address, heads)
}

fn service(address: SocketAddr) -> url::Url {
    format!("http://{address}/")
        .parse()
        .expect("a service address")
}

fn carried_the_token(heads: &Mutex<Vec<String>>) -> bool {
    heads
        .lock()
        .expect("read")
        .iter()
        .any(|head| head.to_ascii_lowercase().contains("\r\nauthorization:"))
}

#[test]
fn only_a_successful_answer_naming_rdownloader_is_the_service() {
    assert!(answers_as_rdownloader(
        StatusCode::OK,
        SERVICE_HEALTH.as_bytes()
    ));
    assert!(!answers_as_rdownloader(
        StatusCode::SERVICE_UNAVAILABLE,
        SERVICE_HEALTH.as_bytes()
    ));
    assert!(!answers_as_rdownloader(
        StatusCode::OK,
        b"<html>It works!</html>"
    ));
    assert!(!answers_as_rdownloader(
        StatusCode::OK,
        br#"{"status":"ok"}"#
    ));
    assert!(!answers_as_rdownloader(
        StatusCode::OK,
        br#"{"service":"JDownloader"}"#
    ));
    assert!(!answers_as_rdownloader(StatusCode::OK, b""));
}

/// The finding itself: a program of another account on the service's port used to receive the
/// token with the first request. It is asked who it is first, and the token never leaves.
#[tokio::test]
async fn a_foreign_listener_is_reported_and_never_sent_the_token() {
    let (address, heads) = listener(r#"{"status":"ok"}"#).await;
    let client =
        CaptureClient::new(service(address), "capture-token".to_owned()).expect("a client");

    let refused = client
        .agent_settings()
        .await
        .expect_err("a foreign listener is not the service");
    assert!(refused.is::<ForeignListener>(), "{refused:?}");
    let refused = client
        .upload_nzb_bytes("release.nzb".to_owned(), b"<nzb/>".to_vec())
        .await
        .expect_err("nor for the NZB hand-over");
    assert!(refused.is::<ForeignListener>(), "{refused:?}");

    assert!(!carried_the_token(&heads), "{:?}", heads.lock());
    assert!(
        heads
            .lock()
            .expect("read")
            .iter()
            .all(|head| head.starts_with("GET /api/v1/health ")),
        "{:?}",
        heads.lock()
    );
}

#[tokio::test]
async fn the_service_is_asked_once_and_then_given_the_token() {
    let (address, heads) = listener(SERVICE_HEALTH).await;
    let client =
        CaptureClient::new(service(address), "capture-token".to_owned()).expect("a client");

    // Refused by the stand-in, which is not the point: what reached it is.
    let _ = client.agent_settings().await;
    let _ = client.summary().await;

    let heads = heads.lock().expect("read").clone();
    assert_eq!(heads.len(), 3, "{heads:?}");
    assert!(heads[0].starts_with("GET /api/v1/health "), "{heads:?}");
    assert!(!heads[0].to_ascii_lowercase().contains("authorization:"));
    assert!(heads[1].starts_with("GET /api/v1/capture/agent-settings "));
    assert!(heads[2].starts_with("GET /api/v1/capture/summary "));
    for head in &heads[1..] {
        assert!(
            head.contains("Bearer capture-token") || head.contains("bearer capture-token"),
            "{head}"
        );
    }
}

/// Confirmed once, the answer holds — until the connection is lost, after which whoever listens
/// is asked again.
#[tokio::test]
async fn a_lost_connection_has_the_next_listener_asked_again() {
    let http = build(Purpose::Request).expect("a client");
    let (real, _) = listener(SERVICE_HEALTH).await;
    let (foreign, _) = listener("<html></html>").await;
    let identity = Identity::default();

    identity
        .confirm(&http, &service(real))
        .await
        .expect("the service answers as itself");
    identity
        .confirm(&http, &service(foreign))
        .await
        .expect("confirmed, so not asked again");
    identity.forget();
    let refused = identity
        .confirm(&http, &service(foreign))
        .await
        .expect_err("asked again after the loss");
    assert!(refused.is::<ForeignListener>(), "{refused:?}");

    // The client forgets on its own when a request cannot connect.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind")
        .local_addr()
        .expect("address");
    let client =
        CaptureClient::confirmed(service(closed), "capture-token".to_owned()).expect("a client");
    let _ = client.agent_settings().await;
    assert!(!client.identity.confirmed.load(Ordering::Acquire));
}

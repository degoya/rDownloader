use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{lock, payload, respond, serve};

/// One `GET` over a fresh connection; the whole answer as text.
async fn get_raw(address: std::net::SocketAddr, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(address).await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await?;
    Ok(answer)
}

#[cfg(feature = "db")]
#[tokio::test]
async fn the_database_is_opened_and_migrated_in_the_given_directory() {
    use super::{DATABASE_FILE, temp_database};

    let directory = tempfile::tempdir().expect("tempdir");
    let database = temp_database(directory.path()).await;
    assert!(directory.path().join(DATABASE_FILE).is_file());
    assert!(
        database.list_packages().await.expect("packages").is_empty(),
        "a fresh database answers its queries"
    );
}

#[tokio::test]
async fn the_server_answers_on_its_address_and_url() {
    let server = serve(Router::new().route("/ping", get(|| async { "pong" }))).await;
    assert!(server.addr().ip().is_loopback());
    assert_eq!(
        server.url("/ping"),
        format!("http://{}/ping", server.addr())
    );
    let answer = get_raw(server.addr(), "/ping").await.expect("answer");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("pong"), "{answer}");
}

#[tokio::test]
async fn the_server_stops_when_its_handle_drops() {
    let server = serve(Router::new().route("/", get(|| async { "up" }))).await;
    let address = server.addr();
    drop(server);
    // The abort takes effect at the task's next poll; the listener goes with it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if get_raw(address, "/").await.is_err() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server still answers after its handle dropped"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn a_poisoned_lock_hands_over_its_value() {
    let shared = Arc::new(Mutex::new(7));
    let poisoner = Arc::clone(&shared);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the lock");
    })
    .join();
    assert!(shared.is_poisoned());
    assert_eq!(*lock(&shared), 7);
}

#[test]
fn a_response_carries_status_headers_and_body() {
    let response = respond(
        StatusCode::PARTIAL_CONTENT,
        &[("content-range", "bytes 0-1/2".to_owned())],
        "ab",
    );
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("content-range")
            .map(|value| value.as_bytes()),
        Some(&b"bytes 0-1/2"[..])
    );
}

#[test]
fn the_payload_repeats_every_251_bytes() {
    let bytes = payload(600);
    assert_eq!(bytes.len(), 600);
    assert_eq!(bytes[0], 0);
    assert_eq!(bytes[250], 250);
    assert_eq!(bytes[251], 0);
    assert_eq!(&bytes[..251], &bytes[251..502]);
}

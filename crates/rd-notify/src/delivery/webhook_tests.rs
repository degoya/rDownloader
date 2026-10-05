use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{send, sign};
use crate::{
    delivery::{Attempt, Message},
    model::{NotificationEvent, NotificationTarget, TargetKind},
};

/// A receiver on loopback that answers every connection with `answer` — followed by chunks of
/// `x` for as long as the caller reads, when `endless` — and counts the connections it took.
async fn receiver(answer: Vec<u8>, endless: bool) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    let taken = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&taken);
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let answer = Arc::clone(&answer);
            tokio::spawn(async move {
                let mut request = [0_u8; 8192];
                let _ = stream.read(&mut request).await;
                if stream.write_all(&answer).await.is_err() {
                    return;
                }
                let chunk = format!("1000\r\n{}\r\n", "x".repeat(4096));
                while endless && stream.write_all(chunk.as_bytes()).await.is_ok() {}
            });
        }
    });
    (port, taken)
}

fn ok_answer() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec()
}

fn target(port: u16) -> NotificationTarget {
    NotificationTarget {
        id: rd_core::NotificationTargetId::new(),
        name: "hook".to_owned(),
        kind: TargetKind::Webhook,
        enabled: true,
        endpoint: format!("http://127.0.0.1:{port}/hook"),
        config: serde_json::json!({}),
        secret_ref: None,
        has_secret: false,
    }
}

async fn post(reach: &rd_http::AddressPolicy, port: u16) -> Attempt {
    let message = Message {
        title: "Package finished".to_owned(),
        body: "example.iso".to_owned(),
        event: NotificationEvent::PackageCompleted,
        idempotency_key: "test:1".to_owned(),
        payload: serde_json::json!({ "event": "package_completed" }),
    };
    send(reach, &target(port), &message, None)
        .await
        .expect("attempt")
}

/// Without the person's word for loopback, a webhook never connects to this machine: the
/// address is refused before the request and the refusal is not retried (audit 2026-10-05, S2).
#[tokio::test]
async fn a_refused_address_is_never_connected_to() {
    let (port, taken) = receiver(ok_answer(), false).await;
    let attempt = post(&rd_http::AddressPolicy::new(true), port).await;
    assert!(!attempt.ok, "{attempt:?}");
    assert!(!attempt.retryable, "{attempt:?}");
    assert!(
        attempt
            .excerpt
            .as_deref()
            .is_some_and(|text| text.contains("may not reach")),
        "{attempt:?}"
    );
    assert_eq!(taken.load(Ordering::SeqCst), 0);
}

/// The owner's rule for an entered address (RA-HOST-01) still holds: a receiver beside the
/// service, on a port that is not one of its own, is reached.
#[tokio::test]
async fn an_entered_loopback_receiver_is_reached() {
    let (port, taken) = receiver(ok_answer(), false).await;
    let attempt = post(&rd_http::AddressPolicy::new(true).with_loopback(), port).await;
    assert!(attempt.ok, "{attempt:?}");
    assert_eq!(taken.load(Ordering::SeqCst), 1);
}

/// A receiver's redirect is reported, not followed: a public address that answers `307` with
/// an inner one is the way around a check made once.
#[tokio::test]
async fn a_redirect_is_not_followed() {
    let (inner, reached) = receiver(ok_answer(), false).await;
    let redirect = format!(
        "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:{inner}/hook\r\n\
         content-length: 0\r\nconnection: close\r\n\r\n"
    );
    let (outer, _) = receiver(redirect.into_bytes(), false).await;
    let attempt = post(&rd_http::AddressPolicy::new(true).with_loopback(), outer).await;
    assert!(!attempt.ok, "{attempt:?}");
    assert_eq!(attempt.status, Some(307));
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

/// A failed answer is read only as far as the history needs it: a receiver that keeps sending
/// neither holds the attempt until the timeout nor fills the memory.
#[tokio::test]
async fn only_the_head_of_a_failed_answer_is_read() {
    let head = b"HTTP/1.1 500 Internal Server Error\r\ntransfer-encoding: chunked\r\n\r\n";
    let (port, _) = receiver(head.to_vec(), true).await;
    let started = Instant::now();
    let attempt = post(&rd_http::AddressPolicy::new(true).with_loopback(), port).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!attempt.ok, "{attempt:?}");
    assert!(attempt.retryable, "{attempt:?}");
    assert_eq!(attempt.status, Some(500));
    assert_eq!(attempt.excerpt.as_deref(), Some("x".repeat(500).as_str()));
}

#[test]
fn the_signature_is_a_stable_hmac_over_the_exact_body() {
    let first = sign("topsecret", b"{\"a\":1}");
    assert_eq!(first, sign("topsecret", b"{\"a\":1}"));
    assert!(first.starts_with("sha256="));
    assert_ne!(first, sign("other", b"{\"a\":1}"));
    assert_ne!(first, sign("topsecret", b"{\"a\":2}"));
    // The secret itself is nowhere in the header value.
    assert!(!first.contains("topsecret"));
}

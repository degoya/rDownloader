use std::time::Duration;

use super::{Deadlines, Detail, Purpose, ServiceRefusal, build, build_with, deadlines, user_agent};
use reqwest::StatusCode;

/// Every client this crate builds has a connection deadline, and only the one that is meant
/// to stay open lacks a total one.
#[test]
fn every_client_carries_the_deadlines_its_purpose_needs() {
    for purpose in [Purpose::Request, Purpose::Stream, Purpose::Health] {
        let policy = deadlines(purpose);
        assert!(
            policy.connect > Duration::ZERO,
            "{purpose:?} must not wait indefinitely for a connection"
        );
        build(purpose).unwrap_or_else(|error| panic!("{purpose:?} builds: {error}"));
    }

    assert_eq!(
        deadlines(Purpose::Request).total,
        Some(Duration::from_secs(30)),
        "a short call that never returns is the failure this exists for"
    );
    assert_eq!(
        deadlines(Purpose::Stream).total,
        None,
        "a total deadline would cut the event stream in normal operation"
    );
    assert_eq!(
        deadlines(Purpose::Stream).read,
        Some(Duration::from_secs(90)),
        "the stream is bounded by the gap between two pieces of data instead"
    );
    assert_eq!(
        deadlines(Purpose::Health).total,
        Some(Duration::from_secs(3))
    );
}

/// Every request names the agent and its version, which is how the service learns which
/// version is connected (RD-190-07).
#[tokio::test]
async fn every_request_carries_the_agents_version() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let address = listener.local_addr().expect("the bound address");
    let (sent, received) = tokio::sync::oneshot::channel::<String>();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut head = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => head.extend_from_slice(&chunk[..read]),
            }
        }
        let _ = stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await;
        let _ = sent.send(String::from_utf8_lossy(&head).to_lowercase());
    });

    // The event stream's client is built the same way (`build_with`), so this one stands
    // for all three purposes.
    let client = build(Purpose::Request).expect("build a client");
    client
        .get(format!("http://{address}/"))
        .send()
        .await
        .expect("the request is answered");
    let head = received.await.expect("the request head");
    assert_eq!(
        user_agent(),
        format!("rdownloader-capture/{}", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        head.contains(&format!("user-agent: {}\r\n", user_agent())),
        "{head}"
    );
}

/// The half-open connection: the handshake completes and nothing ever answers. The request
/// used to sit there for the rest of the process's life.
#[tokio::test]
async fn a_peer_that_never_answers_ends_in_an_error_rather_than_a_hang() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let address = listener.local_addr().expect("the bound address");
    // Accepts the connection and then says nothing at all, which is what a peer that has
    // gone away without a FIN or an RST looks like from here.
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });

    let client = build_with(Deadlines {
        connect: Duration::from_secs(5),
        total: Some(Duration::from_millis(250)),
        read: None,
    })
    .expect("build a client");
    let started = std::time::Instant::now();
    let result = client.get(format!("http://{address}/")).send().await;
    assert!(result.is_err(), "the request has to end, not wait");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it ended after {:?}, which is not within its deadline",
        started.elapsed()
    );
}

#[test]
fn a_refusal_carries_the_stable_code_and_still_reads_as_before() {
    let refusal = ServiceRefusal::new(
        "collector",
        StatusCode::BAD_REQUEST,
        r#"{"error":"All links were skipped by the domain blocklist","code":"collector.all_links_excluded"}"#,
    );
    assert_eq!(refusal.code(), Some("collector.all_links_excluded"));
    assert_eq!(refusal.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        refusal.to_string(),
        concat!(
            "collector returned HTTP 400 Bad Request: ",
            r#"{"error":"All links were skipped by the domain blocklist","code":"collector.all_links_excluded"}"#
        ),
        "the message the logs already carried must not change"
    );
}

#[test]
fn a_body_without_a_code_yields_none_rather_than_a_guess() {
    let empty = ServiceRefusal::new("collector", StatusCode::BAD_GATEWAY, "   ");
    assert_eq!(empty.code(), None);
    assert_eq!(empty.to_string(), "collector returned HTTP 502 Bad Gateway");

    let prose = ServiceRefusal::new("collector", StatusCode::BAD_GATEWAY, "upstream is down");
    assert_eq!(
        prose.code(),
        None,
        "prose is not a code, and nothing may be inferred from its text"
    );
    assert_eq!(
        prose.to_string(),
        "collector returned HTTP 502 Bad Gateway: upstream is down"
    );
}

#[test]
fn a_long_body_still_yields_its_code() {
    let padding = "x".repeat(4096);
    let refusal = ServiceRefusal::new(
        "collector",
        StatusCode::BAD_REQUEST,
        &format!(r#"{{"error":"{padding}","code":"collector.all_links_excluded"}}"#),
    );
    assert_eq!(
        refusal.code(),
        Some("collector.all_links_excluded"),
        "the detail is capped for readability, the code is read from the whole body"
    );
    let prefix = "collector returned HTTP 400 Bad Request: ";
    assert_eq!(
        refusal.to_string().chars().count(),
        prefix.chars().count() + 1024,
        "the readable detail stays capped"
    );
}

/// A refusal that arrives broken is not the same thing as a refusal without a detail, and
/// the two used to be one value (RD-109-10).
#[test]
fn a_body_that_could_not_be_read_is_not_a_body_that_was_empty() {
    let empty = ServiceRefusal::new("collector", StatusCode::SERVICE_UNAVAILABLE, "");
    assert_eq!(empty.detail(), &Detail::Empty);
    assert_eq!(
        empty.to_string(),
        "collector returned HTTP 503 Service Unavailable",
        "the message the logs already carried must not change"
    );

    let broken = ServiceRefusal::unreadable(
        "collector",
        StatusCode::SERVICE_UNAVAILABLE,
        "error decoding response body: connection closed before message completed",
    );
    assert_eq!(
        broken.detail(),
        &Detail::Unreadable(
            "error decoding response body: connection closed before message completed".to_owned()
        )
    );
    assert_ne!(broken.detail(), empty.detail());
    assert!(
        broken.to_string().contains("did not arrive whole"),
        "{broken}"
    );
}

/// Every call now yields a refusal a caller can act on, which is what
/// `decided_submission_code` needs and what the two `bail!` paths destroyed.
#[test]
fn a_refusal_from_any_call_still_carries_its_stable_code() {
    for operation in [
        "collector",
        "event stream",
        "transfer summary",
        "NZB import",
    ] {
        let refusal = ServiceRefusal::new(
            operation,
            StatusCode::BAD_REQUEST,
            r#"{"error":"nothing was a link","code":"collector.no_links_found"}"#,
        );
        assert_eq!(refusal.code(), Some("collector.no_links_found"));
        assert_eq!(refusal.status(), StatusCode::BAD_REQUEST);
        assert!(
            refusal.to_string().starts_with(operation),
            "the log line still names the call: {refusal}"
        );
    }
}

/// Answers one request with `status` and `body` after reading the whole request.
async fn answer_once(status: &'static str, body: &'static str) -> std::net::SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let address = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let text = String::from_utf8_lossy(&request).to_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => request.extend_from_slice(&chunk[..read]),
            }
        }
        let answer = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(answer.as_bytes()).await;
    });
    address
}

/// The cause of the vanishing pick list (RD-1190-17): the intake answers a series page with
/// `site_rules.pick_waiting`, and the agent took that for a failure and handed the clipboard
/// over again and again -- each time listing the page anew. It is a success now.
#[tokio::test]
async fn a_page_waiting_for_a_choice_is_a_success_of_the_hand_over() {
    let address = answer_once(
        "400 Bad Request",
        r#"{"error":"series.example listed 30 entries; choose which of them to resolve","code":"site_rules.pick_waiting","params":{"entries":"30","list":"19a-0","rule":"series.example"}}"#,
    )
    .await;
    let client = super::CaptureClient::confirmed(
        format!("http://{address}/").parse().expect("url"),
        "token".to_owned(),
    )
    .expect("a client");
    let submitted = client
        .submit_links(
            vec!["https://series.example/serie/show/".parse().expect("url")],
            "clipboard",
            None,
            None,
        )
        .await
        .expect("a page waiting for a choice is not a failure");
    assert_eq!(submitted, super::Submitted::PickWaiting(30));
    assert_eq!(
        submitted.notice().as_deref(),
        Some("A page lists 30 releases; choose them in rDownloader's LinkGrabber")
    );

    // Any other refusal stays one.
    let address = answer_once(
        "400 Bad Request",
        r#"{"error":"nothing was a link","code":"collector.no_links_found"}"#,
    )
    .await;
    let client = super::CaptureClient::confirmed(
        format!("http://{address}/").parse().expect("url"),
        "token".to_owned(),
    )
    .expect("a client");
    let refused = client
        .submit_links(
            vec!["https://example.org/".parse().expect("url")],
            "clipboard",
            None,
            None,
        )
        .await
        .expect_err("refused");
    assert_eq!(super::pick_waiting(&refused), None);
}

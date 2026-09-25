//! The agent's half of the event resume (RD-110-23), and with it the case RD-110-22 was cut
//! for: an intake that arrives while the agent is disconnected is still announced.
//!
//! The service is a bare TCP listener answering one scripted response per connection. It holds
//! still exactly what `rd-api` does since RD-110-23 -- it reads `Last-Event-ID` and hands back
//! what came after it -- so these tests cover the agent alone; the service side has its own
//! tests in `rd-api`. Nothing here needs the real service, which keeps this crate's tests at a
//! second rather than behind the rd-api link.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

use crate::{
    client::CaptureClient,
    notify::{Intake, read_stream},
};

/// A `retry:` far below the agent's own first backoff, so a test can tell the two apart.
const FIRST: &str = "retry: 50\nid: 1\nevent: collector.intake\ndata: {\"payload\":{\"candidate_count\":1,\"package_count\":1}}\n\n";
const MISSED: &str = "id: 2\nevent: collector.intake\ndata: {\"payload\":{\"candidate_count\":3,\"package_count\":1}}\n\n";
const EXPIRED: &str = "event: stream.expired\ndata: {\"last_event_id\":\"1\"}\n\n";

/// What the scripted service does with one connection, after the response head.
enum Answer {
    /// Sends the bytes and closes: what a dropped connection looks like from the agent.
    Close(&'static str),
    /// Sends `missed` only when the agent resumed from `after`, then holds the connection
    /// open -- the service's behaviour once it keeps a buffer.
    Resume {
        after: &'static str,
        missed: &'static str,
    },
    /// Sends the bytes and holds the connection open.
    Hold(&'static str),
}

struct Service {
    address: SocketAddr,
    /// The `Last-Event-ID` of every connection, in order; `None` where the agent sent none.
    last_event_ids: Arc<Mutex<Vec<Option<String>>>>,
}

async fn serve(script: Vec<Answer>) -> Service {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let address = listener.local_addr().expect("the bound address");
    let last_event_ids = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&last_event_ids);
    tokio::spawn(async move {
        let mut held = Vec::new();
        for answer in script {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let head = read_head(&mut stream).await;
            let last_event_id = header(&head, "last-event-id");
            seen.lock().expect("record").push(last_event_id.clone());
            let body = match &answer {
                Answer::Close(bytes) | Answer::Hold(bytes) => *bytes,
                Answer::Resume { after, missed } => {
                    if last_event_id.as_deref() == Some(after) {
                        *missed
                    } else {
                        ""
                    }
                }
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n{body}"
            );
            if stream.write_all(response.as_bytes()).await.is_err() {
                return;
            }
            match answer {
                Answer::Close(_) => drop(stream),
                Answer::Resume { .. } | Answer::Hold(_) => held.push(stream),
            }
        }
        // The script is used up: refuse further connections, keep the held ones open.
        drop(listener);
        let _open = held;
        std::future::pending::<()>().await;
    });
    Service {
        address,
        last_event_ids,
    }
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

fn header(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (field, value) = line.split_once(':')?;
        field
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

fn watch(
    service: &Service,
) -> (
    mpsc::Receiver<Intake>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let client = CaptureClient::new(
        format!("http://{}/", service.address)
            .parse()
            .expect("a service address"),
        "capture-token".to_owned(),
    )
    .expect("a client");
    let (sender, receiver) = mpsc::channel(8);
    let cancellation = CancellationToken::new();
    let reading = tokio::spawn(read_stream(client, cancellation.clone(), sender));
    (receiver, cancellation, reading)
}

async fn next(intakes: &mut mpsc::Receiver<Intake>, patience: Duration) -> Option<Intake> {
    tokio::time::timeout(patience, intakes.recv())
        .await
        .ok()
        .flatten()
}

fn ids(service: &Service) -> Vec<Option<String>> {
    service.last_event_ids.lock().expect("read").clone()
}

/// RD-110-22: the intake sent while the agent was away is announced after the reconnect. It
/// only can be because the agent asks for it: the second connection carries the id it holds,
/// and the reconnect comes after the `retry:` the service sent rather than the agent's own
/// two-second first backoff.
#[tokio::test]
async fn an_intake_during_a_disconnect_is_announced_after_the_reconnect() {
    let service = serve(vec![
        Answer::Close(FIRST),
        Answer::Resume {
            after: "1",
            missed: MISSED,
        },
    ])
    .await;
    let (mut intakes, cancellation, reading) = watch(&service);

    let first = next(&mut intakes, Duration::from_secs(5))
        .await
        .expect("the intake on the first connection");
    assert_eq!(first.candidate_count, 1);
    let dropped = std::time::Instant::now();

    let second = next(&mut intakes, Duration::from_secs(5))
        .await
        .expect("the intake sent while the agent was disconnected");
    assert_eq!(second.candidate_count, 3);
    assert!(
        dropped.elapsed() < Duration::from_secs(1),
        "the reconnect took {:?}: the service's retry of 50 ms was not obeyed",
        dropped.elapsed()
    );
    assert_eq!(
        ids(&service),
        vec![None, Some("1".to_owned())],
        "the first connection resumes from nothing, the second from the id the agent holds"
    );

    cancellation.cancel();
    reading.await.expect("the reader ends on cancellation");
}

/// RD-110-22, the other half: a reconnect with nothing missed announces nothing. A toast on
/// every reconnect would be worse than the loss it replaces.
#[tokio::test]
async fn a_reconnect_without_missed_intakes_announces_nothing() {
    let service = serve(vec![
        Answer::Close(FIRST),
        Answer::Resume {
            after: "1",
            missed: "",
        },
    ])
    .await;
    let (mut intakes, cancellation, reading) = watch(&service);

    let first = next(&mut intakes, Duration::from_secs(5))
        .await
        .expect("the intake on the first connection");
    assert_eq!(first.candidate_count, 1);
    assert!(
        next(&mut intakes, Duration::from_millis(600))
            .await
            .is_none(),
        "a reconnect with nothing missed produced a notification"
    );
    assert_eq!(
        ids(&service),
        vec![None, Some("1".to_owned())],
        "the agent reconnected with the id it holds"
    );

    cancellation.cancel();
    reading.await.expect("the reader ends on cancellation");
}

/// A service that cannot resume -- restarted, or the id fell out of its buffer -- says so, and
/// the agent takes it as a log line, not as a toast: it has no way to ask what it missed.
#[tokio::test]
async fn an_expired_resume_point_is_not_announced_as_an_intake() {
    let service = serve(vec![Answer::Close(FIRST), Answer::Hold(EXPIRED)]).await;
    let (mut intakes, cancellation, reading) = watch(&service);

    let first = next(&mut intakes, Duration::from_secs(5))
        .await
        .expect("the intake on the first connection");
    assert_eq!(first.candidate_count, 1);
    assert!(
        next(&mut intakes, Duration::from_millis(600))
            .await
            .is_none(),
        "the expiry marker was announced as an intake"
    );
    assert_eq!(ids(&service).len(), 2, "the agent did reconnect");

    cancellation.cancel();
    reading.await.expect("the reader ends on cancellation");
}

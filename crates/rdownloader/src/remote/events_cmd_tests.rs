use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use super::{EVENTS_PATH, Ended, Follower, render, to_the_second};
use crate::remote::{
    Client, CommandError, Failure, Format,
    sse::{Frame, Parser},
};

const FIRST_ID: &str = "0192f1a0-5b6e-7c3d-8e9f-0a1b2c3d4e5f";
const SECOND_ID: &str = "0192f1a0-5b6e-7c3d-8e9f-0a1b2c3d4e60";
const STATE_DATA: &str = r#"{"id":"0192f1a0-5b6e-7c3d-8e9f-0a1b2c3d4e5f","kind":"download_state","occurred_at":"2026-10-10T08:15:02.123456789Z","payload":{"id":"0192f19f-0000-7000-8000-000000000001","state":"downloading"}}"#;
const PROGRESS_DATA: &str = r#"{"id":"0192f1a0-5b6e-7c3d-8e9f-0a1b2c3d4e60","kind":"download_progress","occurred_at":"2026-10-10T08:15:03Z","payload":{"downloaded_bytes":1048576,"id":"0192f19f-0000-7000-8000-000000000001","total_bytes":4194304}}"#;

/// A stream as `GET /api/v1/events` sends it, field for field as axum writes it: the opening
/// `retry:`, an event, a keep-alive comment, the lag marker without an id, and one more event.
/// The payloads' keys are in alphabetical order, so the output is the same whichever map
/// `serde_json` was built with.
fn recorded() -> String {
    format!(
        "retry: 5000\n\n\
         id: {FIRST_ID}\nevent: download.state\ndata: {STATE_DATA}\n\n\
         :\n\n\
         event: stream.lagged\ndata: {{\"dropped\":7}}\n\n\
         id: {SECOND_ID}\nevent: download.progress\ndata: {PROGRESS_DATA}\n\n"
    )
}

fn expected_text() -> String {
    format!(
        "2026-10-10T08:15:02Z  download.state  \
         {{\"id\":\"0192f19f-0000-7000-8000-000000000001\",\"state\":\"downloading\"}}\n\
         {:22}stream.lagged  {{\"dropped\":7}}\n\
         2026-10-10T08:15:03Z  download.progress  \
         {{\"downloaded_bytes\":1048576,\"id\":\"0192f19f-0000-7000-8000-000000000001\",\
         \"total_bytes\":4194304}}\n",
        ""
    )
}

fn frames(chunks: &[&[u8]]) -> Vec<Frame> {
    let mut parser = Parser::default();
    chunks
        .iter()
        .flat_map(|chunk| parser.feed(chunk).expect("parse"))
        .collect()
}

#[test]
fn the_recorded_stream_parses_the_same_whole_and_byte_by_byte() {
    let recording = recorded();
    let whole = frames(&[recording.as_bytes()]);
    let bytes: Vec<&[u8]> = recording.as_bytes().chunks(1).collect();
    assert_eq!(frames(&bytes), whole);
    assert_eq!(
        whole,
        vec![
            Frame {
                retry: Some(Duration::from_secs(5)),
                ..Frame::default()
            },
            Frame {
                event: Some("download.state".to_owned()),
                data: Some(STATE_DATA.to_owned()),
                id: Some(FIRST_ID.to_owned()),
                retry: None,
            },
            Frame {
                event: Some("stream.lagged".to_owned()),
                data: Some(r#"{"dropped":7}"#.to_owned()),
                ..Frame::default()
            },
            Frame {
                event: Some("download.progress".to_owned()),
                data: Some(PROGRESS_DATA.to_owned()),
                id: Some(SECOND_ID.to_owned()),
                retry: None,
            },
        ]
    );
}

/// The specification's line endings, including a `\r\n` that a chunk boundary splits: read as
/// two endings, the `\n` would end the frame between the two `data:` lines.
#[test]
fn every_line_ending_and_a_split_one_read_as_the_specification_says() {
    let parsed = frames(&[b"data: a\r\ndata:b\r\r: comment\ndata:  c\n\n"]);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].data.as_deref(), Some("a\nb"));
    assert_eq!(
        parsed[1].data.as_deref(),
        Some(" c"),
        "only one space is dropped"
    );

    let split = frames(&[b"data: x\r", b"\ndata: y\n\n"]);
    assert_eq!(split.len(), 1, "{split:?}");
    assert_eq!(split[0].data.as_deref(), Some("x\ny"));
}

#[test]
fn events_and_markers_render_as_one_line_each() {
    assert_eq!(
        render(Format::Json, Some("download.state"), STATE_DATA),
        STATE_DATA,
        "an event is the server's envelope"
    );
    assert_eq!(
        render(Format::Json, Some("stream.lagged"), r#"{"dropped":7}"#),
        r#"{"kind":"stream.lagged","payload":{"dropped":7}}"#,
        "a marker gets the same two keys"
    );
    let multi_line = render(
        Format::Json,
        Some("download.state"),
        "{\n\"kind\": \"x\"\n}",
    );
    assert_eq!(
        multi_line, r#"{"kind":"x"}"#,
        "JSON Lines: one line per event"
    );

    let long = format!(
        r#"{{"kind":"system","occurred_at":"2026-10-10T08:15:02Z","payload":"{}"}}"#,
        "x".repeat(400)
    );
    let line = render(Format::Text, Some("system"), &long);
    assert!(line.ends_with('…'), "{line}");
    assert!(line.chars().count() < 200, "{line}");
}

#[test]
fn a_time_is_shown_to_the_second() {
    assert_eq!(
        to_the_second("2026-10-10T08:15:02.123456789Z"),
        "2026-10-10T08:15:02Z"
    );
    assert_eq!(
        to_the_second("2026-10-10T08:15:02Z"),
        "2026-10-10T08:15:02Z"
    );
    assert_eq!(
        to_the_second("2026-10-10T08:15:02.5+02:00"),
        "2026-10-10T08:15:02.5+02:00",
        "a form it does not know stays as it is"
    );
}

/// Serves `body` as the event stream and records the `Last-Event-ID` of every request.
async fn recorded_server(body: String) -> (String, Arc<Mutex<Vec<Option<String>>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    let router = axum::Router::new().route(
        EVENTS_PATH,
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let record = Arc::clone(&record);
            let body = body.clone();
            async move {
                record.lock().expect("lock").push(
                    headers
                        .get("last-event-id")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned),
                );
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (format!("http://{address}"), seen)
}

#[tokio::test]
async fn a_recorded_stream_is_printed_and_resumed_from_its_last_id() {
    let (server, seen) = recorded_server(recorded()).await;
    let client = Client::streaming(&server, Some("token".to_owned()), 5).expect("client");
    let mut follower = Follower::new(Format::Text);

    let mut out = Vec::new();
    let ended = follower
        .connection(&client, &mut out)
        .await
        .expect("stream");
    assert_eq!(ended, Ended::Server);
    assert_eq!(String::from_utf8(out).expect("utf-8"), expected_text());
    assert_eq!(follower.retry, Duration::from_secs(5));
    assert_eq!(
        follower.last_id.as_deref(),
        Some(SECOND_ID),
        "the marker carries no id and moves nothing"
    );

    follower
        .connection(&client, &mut Vec::new())
        .await
        .expect("second connection");
    assert_eq!(
        *seen.lock().expect("lock"),
        vec![None, Some(SECOND_ID.to_owned())]
    );
}

#[tokio::test]
async fn the_json_form_is_one_document_per_line() {
    let (server, _) = recorded_server(recorded()).await;
    let client = Client::streaming(&server, None, 5).expect("client");
    let mut out = Vec::new();
    Follower::new(Format::Json)
        .connection(&client, &mut out)
        .await
        .expect("stream");
    let text = String::from_utf8(out).expect("utf-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            STATE_DATA,
            r#"{"kind":"stream.lagged","payload":{"dropped":7}}"#,
            PROGRESS_DATA,
        ]
    );
}

/// `| head` closes the output; the follower stops quietly instead of failing on it.
#[tokio::test]
async fn a_closed_output_ends_the_command_without_an_error() {
    struct Closed;
    impl std::io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (server, seen) = recorded_server(recorded()).await;
    let client = Client::streaming(&server, None, 5).expect("client");
    Follower::new(Format::Text)
        .run(&client, &mut Closed)
        .await
        .expect("a closed output is no failure");
    assert_eq!(
        seen.lock().expect("lock").len(),
        1,
        "and nothing reconnects"
    );
}

#[tokio::test]
async fn a_refusal_or_a_dead_address_ends_the_command_with_its_exit_code() {
    let router = axum::Router::new().route(
        EVENTS_PATH,
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({ "code": "auth.required", "error": "Sign in" })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });

    let failure_of = |error: anyhow::Error| {
        error
            .downcast_ref::<CommandError>()
            .map(|command| command.failure)
    };
    let refused = Client::streaming(&format!("http://{address}"), Some("wrong".to_owned()), 5)
        .expect("client");
    let error = Follower::new(Format::Text)
        .run(&refused, &mut Vec::new())
        .await
        .expect_err("a refusal is not retried");
    assert!(error.to_string().contains("auth.required"), "{error}");
    assert_eq!(failure_of(error), Some(Failure::Unauthorized));

    // Port 1 is reserved and nothing listens on it.
    let dead = Client::streaming("http://127.0.0.1:1", None, 5).expect("client");
    let error = Follower::new(Format::Text)
        .run(&dead, &mut Vec::new())
        .await
        .expect_err("a first connection that fails is not retried");
    assert_eq!(failure_of(error), Some(Failure::Unreachable));
}

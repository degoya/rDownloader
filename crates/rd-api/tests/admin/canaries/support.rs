//! What the canary suite is built from: the planted values, the search, the requests with the
//! full-access bearer, the event stream and the log capture it keeps, and the fixtures that
//! refuse every login.

use crate::common;

use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use rd_diagnostics::capture::LogStream;
use rd_provider_registry::{
    CredentialKind, DynamicProvider, ProviderKind, ProviderSource, ProviderSpec, SecretFilledBy,
    SecretSlot, TransferAuth, replace_dynamic,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tower::ServiceExt;

use super::CANARIES;

/// The canaries, and everything else a template may name.
pub(super) struct Planted {
    pub(super) canaries: Vec<(&'static str, String)>,
    pub(super) known: Vec<(String, String)>,
}

impl Planted {
    pub(super) fn new() -> Self {
        let canaries = CANARIES
            .iter()
            .map(|name| (*name, format!("CANARY-{name}-{}", rd_core::EventId::new())))
            .collect();
        Self {
            canaries,
            known: Vec::new(),
        }
    }

    pub(super) fn canary(&self, name: &str) -> String {
        self.expand(&format!("{{{name}}}"))
    }

    /// `template` with every `{name}` replaced.
    pub(super) fn expand(&self, template: &str) -> String {
        let text = self
            .canaries
            .iter()
            .fold(template.to_owned(), |text, (name, value)| {
                text.replace(&format!("{{{name}}}"), value)
            });
        self.known.iter().fold(text, |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
    }
}

/// Everything read, by where it was read.
#[derive(Default)]
pub(super) struct Scan(pub(super) Vec<(String, Vec<u8>)>);

impl Scan {
    pub(super) fn record(&mut self, place: impl Into<String>, bytes: Vec<u8>) {
        self.0.push((place.into(), bytes));
    }

    /// Whether a place whose name starts with `prefix` contains `needle`.
    pub(super) fn shows(&self, prefix: &str, needle: &str) -> bool {
        self.0
            .iter()
            .any(|(place, bytes)| place.starts_with(prefix) && contains(bytes, needle.as_bytes()))
    }

    /// `<canary> in <place>` for every hit.
    pub(super) fn leaks(&self, planted: &Planted) -> Vec<String> {
        let mut found = Vec::new();
        for (place, bytes) in &self.0 {
            // Every value but the minted token starts alike, so one pass tells whether a place
            // needs the others searched at all.
            let marked = contains(bytes, b"CANARY-");
            for (name, value) in &planted.canaries {
                if (marked || !value.starts_with("CANARY-")) && contains(bytes, value.as_bytes()) {
                    found.push(format!("{name} in {place}"));
                }
            }
        }
        found
    }
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Bundles are written below the process-wide data directory, a `OnceLock` the first caller
/// owns; the same arrangement as the diagnostics suite.
pub(super) fn data_directory() {
    static DATA: OnceLock<tempfile::TempDir> = OnceLock::new();
    let directory = DATA.get_or_init(|| tempfile::tempdir().expect("data directory"));
    rd_core::set_data_directory(directory.path());
}

/// Two hosters whose plugin declares a secret slot: one takes an API key, one a login.
pub(super) fn install_providers() {
    let provider =
        |slug: &str, credentials: CredentialKind, username_required: bool| DynamicProvider {
            plugin_id: format!("plugin-{slug}"),
            spec: ProviderSpec {
                slug: slug.to_owned(),
                display_name: format!("Canary {slug}"),
                kind: ProviderKind::Hoster,
                credentials,
                username_required,
                transfer_auth: TransferAuth::None,
                secrets: vec![SecretSlot {
                    reference: format!("{slug}_secret"),
                    domains: vec![format!("api.{slug}.test")],
                    mode: None,
                    filled_by: SecretFilledBy::Person,
                }],
                request_domains: vec![format!("{slug}.test")],
                cookie_scope: None,
                match_hosts: vec![format!("{slug}.test")],
                host_aliases: Vec::new(),
                source: ProviderSource::Plugin,
                plugin_id: Some(format!("plugin-{slug}")),
                plugin_version: Some("1.0.0".to_owned()),
            },
        };
    replace_dynamic(vec![
        provider("canarykey", CredentialKind::ApiKey, false),
        provider("canarylogin", CredentialKind::UsernamePassword, true),
    ]);
}

/// One request with the full-access bearer.
pub(super) async fn rest(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let builder = common::request_to(method, uri).header(
        header::AUTHORIZATION,
        format!("Bearer {}", common::API_BEARER),
    );
    let request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    let (status, _, bytes) = common::send_raw(router, request).await;
    (status, bytes.to_vec())
}

/// A request that has to succeed; its answer is searched like any other place. `Null` sends no
/// body.
pub(super) async fn expect_ok(
    router: &Router,
    scan: &mut Scan,
    method: &str,
    uri: &str,
    body: Value,
) -> Value {
    let body = (!body.is_null()).then_some(body);
    let (status, bytes) = rest(router, method, uri, body).await;
    let answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert!(status.is_success(), "{method} {uri}: {status} {answer}");
    scan.record(format!("{method} {uri} (answer)"), bytes);
    answer
}

/// Opens the web event stream and keeps every byte it sends until the task is stopped.
pub(super) async fn listen(router: &Router) -> (Arc<Mutex<Vec<u8>>>, tokio::task::JoinHandle<()>) {
    let request = common::request_to("GET", "/api/v1/events")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", common::API_BEARER),
        )
        .body(Body::empty())
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let heard: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&heard);
    let task = tokio::spawn(async move {
        while let Some(Ok(frame)) = body.frame().await {
            if let Some(data) = frame.data_ref() {
                sink.lock().expect("stream").extend_from_slice(data);
            }
        }
    });
    (heard, task)
}

/// Stores what the capture took since the last call, and keeps it for the search.
pub(super) async fn drain(stream: &mut LogStream, database: &rd_db::Database, lines: &mut Vec<u8>) {
    let records = stream.drain_ready();
    if records.is_empty() {
        return;
    }
    for record in &records {
        lines.extend(serde_json::to_vec(record).expect("record"));
        lines.push(b'\n');
    }
    database
        .append_log_records(records)
        .await
        .expect("store captured lines");
}

fn mcp_request(session: Option<&str>, body: String) -> Request<Body> {
    let mut builder = common::request_to("POST", "/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", common::API_BEARER),
        );
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body)).expect("request")
}

/// Completes the MCP handshake and returns the session id.
pub(super) async fn mcp_session(router: &Router) -> String {
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"canaries","version":"0.0.0"}}}"#;
    let request = mcp_request(None, initialize.to_owned());
    let response = router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK, "initialize");
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session id")
        .to_owned();
    let ack = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_owned();
    let (status, _, _) = common::send_raw(router, mcp_request(Some(&session), ack)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    session
}

/// Calls one tool; the whole answer, and whether the tool answered rather than refused.
pub(super) async fn call_tool(
    router: &Router,
    session: &str,
    index: usize,
    tool: &str,
    arguments: &str,
) -> (bool, Vec<u8>) {
    let id = 100 + index;
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#
    );
    let (status, headers, bytes) = common::send_raw(router, mcp_request(Some(session), body)).await;
    let streamed = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let payload = if streamed {
        let data = text
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("data: "));
        data.unwrap_or_default().to_owned()
    } else {
        text
    };
    let answer: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
    let answered = status == StatusCode::OK
        && !answer.is_null()
        && answer["error"].is_null()
        && answer["result"]["isError"] != true;
    (answered, bytes.to_vec())
}

/// An HTTP server refusing every request the way S3 refuses a wrong key; the webhook and the
/// object storage check both end here.
pub(super) async fn refusing_http() -> String {
    let app = Router::new().fallback(|| async {
        (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "application/xml")],
            "<?xml version=\"1.0\"?><Error><Code>InvalidAccessKeyId</Code></Error>",
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}")
}

/// An FTP server that greets, asks for a password and refuses it.
pub(super) async fn refusing_ftp() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = tokio::io::BufReader::new(read).lines();
                if write.write_all(b"220 canary fixture\r\n").await.is_err() {
                    return;
                }
                while let Ok(Some(line)) = lines.next_line().await {
                    let command = line
                        .split(' ')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    let answer: &[u8] = match command.as_str() {
                        "USER" => b"331 password required\r\n",
                        "PASS" => b"530 login incorrect\r\n",
                        "QUIT" => b"221 bye\r\n",
                        _ => b"502 not implemented\r\n",
                    };
                    if write.write_all(answer).await.is_err() || command == "QUIT" {
                        return;
                    }
                }
            });
        }
    });
    port
}

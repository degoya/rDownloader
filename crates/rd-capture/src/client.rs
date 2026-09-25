use std::{fmt, time::Duration};

use anyhow::{Context, Result};
use rd_core::IngressSource;
use reqwest::{Client, StatusCode, multipart};
use url::Url;

/// What a client of this crate is going to be used for.
///
/// One `timeout` number does not fit every call this agent makes, which is why the agent used to
/// have none at all: the event stream is meant to stay open for the whole run and a total
/// deadline would cut it in normal operation, while a five-second poll that never returns is
/// the single most expensive failure in the agent -- the task simply stops, forever, with no log
/// line and no restart, while the tray keeps reporting "healthy" (RD-109-06).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// The short JSON and multipart calls. A total deadline belongs on these.
    Request,
    /// The capture event stream. Bounded by the gap between two pieces of data rather than by
    /// its lifetime: the service's keep-alive holds that gap open, and a keep-alive that stops
    /// arriving is exactly the case the deadline is meant to catch.
    Stream,
    /// The tray's health poll. Its answer is only worth having while it is fresh, and the next
    /// one is five seconds away.
    ///
    /// Only the tray constructs this, and the tray is compiled on Windows and macOS only -- so
    /// on Linux the variant has no non-test caller, which is the platform split rather than an
    /// oversight.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    Health,
}

/// The deadlines a client carries, kept as data so the policy itself can be asserted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadlines {
    /// How long establishing the connection may take. Every client of this crate has one.
    pub connect: Duration,
    /// How long the whole request may take. `None` only where a request is meant to be long.
    pub total: Option<Duration>,
    /// Longest gap between two pieces of a response body.
    pub read: Option<Duration>,
}

/// The one policy, per purpose.
#[must_use]
pub fn deadlines(purpose: Purpose) -> Deadlines {
    match purpose {
        Purpose::Request => Deadlines {
            connect: Duration::from_secs(5),
            total: Some(Duration::from_secs(30)),
            read: None,
        },
        Purpose::Stream => Deadlines {
            connect: Duration::from_secs(5),
            total: None,
            read: Some(Duration::from_secs(90)),
        },
        Purpose::Health => Deadlines {
            connect: Duration::from_secs(3),
            total: Some(Duration::from_secs(3)),
            read: None,
        },
    }
}

/// The only place this crate builds a `reqwest::Client`.
///
/// Four hand-written builders used to sit in two files, three of them with no deadline of any
/// kind and the fourth throwing its deadline away on `unwrap_or_default()`. A failure here is
/// reported to the caller rather than replaced by a client without a policy: a client that
/// cannot be built is a fault worth a line in the log, and silently substituting one that hangs
/// forever is the opposite of the property the expression was written for.
pub fn build(purpose: Purpose) -> Result<Client> {
    build_with(deadlines(purpose))
}

/// Applies one set of deadlines. Separate from [`build`] so a test can drive a short budget
/// without waiting out the real one.
fn build_with(deadlines: Deadlines) -> Result<Client> {
    let mut builder = Client::builder()
        .no_proxy()
        .connect_timeout(deadlines.connect);
    if let Some(total) = deadlines.total {
        builder = builder.timeout(total);
    }
    if let Some(read) = deadlines.read {
        builder = builder.read_timeout(read);
    }
    builder.build().context("build the capture HTTP client")
}

/// The service's own answer to a request it refused.
///
/// `ensure_success` used to flatten a failed response into a formatted string, which left every
/// caller with prose and nothing it could act on. REST errors carry a stable `code` for exactly
/// this purpose, so the code is carried out of the body alongside the status, and the formatted
/// message is kept unchanged as the `Display` form: every log line that reports one of these
/// reads exactly as it did before.
///
/// It travels inside `anyhow::Error`, so a caller that does not care is unaffected and one that
/// does recovers it with `downcast_ref`.
#[derive(Debug, Clone)]
pub struct ServiceRefusal {
    operation: String,
    status: StatusCode,
    code: Option<String>,
    detail: Detail,
}

/// What became of the body of a refused response.
///
/// "The service sent nothing" and "the answer broke off on the way" used to be the same value:
/// the body was read with `unwrap_or_default()`, so a 503 whose body was cut off by a dropped
/// connection, a timeout mid-body or a proxy closing read exactly like a correct, detail-free
/// refusal -- and the hint about the connection, the only thing that would have helped, was
/// gone (RD-109-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    /// Read, and it said something.
    Text(String),
    /// Read, and it was empty.
    Empty,
    /// Could not be read; carries what went wrong.
    Unreadable(String),
}

impl ServiceRefusal {
    /// Builds a refusal from the response body, keeping the message the logs already carried.
    ///
    /// The code is read from the untruncated body; only the human-readable detail is capped,
    /// so a long body cannot cut the code out of the very document that carries it.
    pub fn new(operation: &str, status: StatusCode, body: &str) -> Self {
        let code = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|body| {
                body.get("code")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
        let detail = if body.trim().is_empty() {
            Detail::Empty
        } else {
            Detail::Text(body.chars().take(1024).collect())
        };
        Self {
            operation: operation.to_owned(),
            status,
            code,
            detail,
        }
    }

    /// The service answered, but the answer did not arrive whole.
    pub fn unreadable(operation: &str, status: StatusCode, cause: &str) -> Self {
        Self {
            operation: operation.to_owned(),
            status,
            code: None,
            detail: Detail::Unreadable(cause.chars().take(1024).collect()),
        }
    }

    /// The stable `code` the service sent, when the body carried one.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// The HTTP status the service answered with.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// What became of the body.
    pub fn detail(&self) -> &Detail {
        &self.detail
    }
}

impl fmt::Display for ServiceRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operation = &self.operation;
        let status = self.status;
        match &self.detail {
            Detail::Text(detail) => {
                write!(formatter, "{operation} returned HTTP {status}: {detail}")
            }
            Detail::Empty => write!(formatter, "{operation} returned HTTP {status}"),
            Detail::Unreadable(cause) => write!(
                formatter,
                "{operation} returned HTTP {status}, but the answer did not arrive whole: {cause}"
            ),
        }
    }
}

impl std::error::Error for ServiceRefusal {}

#[derive(Clone)]
pub struct CaptureClient {
    service: Url,
    token: String,
    /// The short calls, with a total deadline.
    http: Client,
    /// The event stream, which has none. Two clients rather than one, because the two kinds of
    /// call want opposite policies and a single client could only ever be wrong for one of them.
    stream: Client,
}

impl CaptureClient {
    pub fn new(service: Url, token: String) -> Result<Self> {
        Ok(Self {
            service,
            token,
            http: build(Purpose::Request)?,
            stream: build(Purpose::Stream)?,
        })
    }

    pub async fn submit_links(
        &self,
        urls: Vec<Url>,
        source: &str,
        package_name: Option<&str>,
        password: Option<&str>,
    ) -> Result<()> {
        let source = match source {
            "clipboard" => IngressSource::Clipboard,
            "click_and_load" => IngressSource::ClickAndLoad,
            _ => IngressSource::Api,
        };
        let endpoint = self.service.join("api/v1/capture/batches")?;
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(&self.token)
            .json(&serde_json::json!({
                "text": urls.iter().map(Url::as_str).collect::<Vec<_>>().join("\n"),
                "source": source,
                "source_label": "Capture-Agent",
                "package_name": package_name,
                "password": password
            }))
            .send()
            .await?;
        ensure_success(response, "collector").await?;
        Ok(())
    }

    /// Figures for the tray: how much is running, and how far along.
    ///
    /// Counts, byte totals and the queue's rate — the capture token is a narrow credential, and
    /// the service answers this one accordingly. Nothing here names a file, a folder or an
    /// account.
    pub async fn summary(&self) -> anyhow::Result<crate::activity::Summary> {
        let url = self.service.join("api/v1/capture/summary")?;
        let response = self.http.get(url).bearer_auth(&self.token).send().await?;
        let response = ensure_success(response, "transfer summary").await?;
        Ok(response.json().await?)
    }

    /// Opens the capture-scoped event stream the agent's watchers listen on.
    ///
    /// The response is returned unread: it stays open for as long as the agent runs. The
    /// stream carries `collector.intake` for the desktop notifications and nothing the agent
    /// acts on besides — a capture token is a narrow credential.
    ///
    /// `last_event_id` is the id of the last frame the previous connection delivered. Sent as
    /// `Last-Event-ID`, it asks the service for everything after it (RD-110-23); on the first
    /// connection there is nothing to ask for and no header goes out.
    pub async fn capture_events(&self, last_event_id: Option<&str>) -> Result<reqwest::Response> {
        let endpoint = self.service.join("api/v1/capture/events")?;
        let mut request = self
            .stream
            .get(endpoint)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("Last-Event-ID", id);
        }
        let response = request.send().await?;
        ensure_success(response, "event stream").await
    }

    pub async fn upload_nzb_bytes(&self, name: String, content: Vec<u8>) -> Result<()> {
        let form = multipart::Form::new().part(
            "file",
            multipart::Part::bytes(content)
                .file_name(name)
                .mime_str("application/x-nzb")?,
        );
        let response = self
            .http
            .post(self.service.join("api/v1/capture/nzb")?)
            .bearer_auth(&self.token)
            .multipart(form)
            .send()
            .await?;
        ensure_success(response, "NZB import").await?;
        Ok(())
    }
}

/// The one shape a refused response takes in this crate.
///
/// Every call goes through here now. `capture_events` used to flatten a refusal into a
/// formatted string, and `summary` used `error_for_status`, which never reads the body at all
/// -- so the stable `code` the REST surface carries for exactly this purpose reached only part
/// of the calls, and `decided_submission_code` could only ever fire for those (RD-109-10).
///
/// It once took a second argument as well: a secret the answer was not allowed to quote back,
/// which existed for the single-use widget token the captcha window handed over. That call is
/// gone with the window (RD-109-11), and nothing this agent sends now travels in a request
/// body that a refusal could echo, so the guard went with its only caller rather than staying
/// as a parameter permanently passed `None`.
///
/// Returns the response so a caller that wants the body on success still has it.
async fn ensure_success(response: reqwest::Response, operation: &str) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let refusal = match response.text().await {
        Ok(body) => ServiceRefusal::new(operation, status, &body),
        // Not "the service sent no detail": the answer broke off, and that is the one piece of
        // information worth having here.
        Err(error) => ServiceRefusal::unreadable(operation, status, &error.to_string()),
    };
    Err(refusal.into())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Deadlines, Detail, Purpose, ServiceRefusal, build, build_with, deadlines};
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
                "error decoding response body: connection closed before message completed"
                    .to_owned()
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
}

use std::{fmt, time::Duration};

use anyhow::{Context, Result};
use rd_core::IngressSource;
use reqwest::{Client, StatusCode, multipart};
use url::Url;

mod identity;
mod linkgrabber;
mod self_update;

pub(crate) use identity::ForeignListener;
// The tray's server line reads the version from the health answer (RD-1240-06).
use identity::Identity;
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) use identity::health_version;
#[cfg(any(windows, target_os = "macos"))]
pub(crate) use identity::read_health_answer;

/// What a client of this crate is going to be used for.
///
/// One `timeout` number does not fit every call this agent makes, which is why the agent used to
/// have none at all: the event stream is meant to stay open for the whole run and a total
/// deadline would cut it in normal operation, while a five-second poll that never returns is
/// the single most expensive failure in the agent -- the task simply stops, forever, with no log
/// line and no restart, while the tray keeps reporting "healthy" (RD-109-06).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
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
pub(crate) struct Deadlines {
    /// How long establishing the connection may take. Every client of this crate has one.
    pub connect: Duration,
    /// How long the whole request may take. `None` only where a request is meant to be long.
    pub total: Option<Duration>,
    /// Longest gap between two pieces of a response body.
    pub read: Option<Duration>,
}

/// The one policy, per purpose.
#[must_use]
pub(crate) fn deadlines(purpose: Purpose) -> Deadlines {
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
pub(crate) fn build(purpose: Purpose) -> Result<Client> {
    build_with(deadlines(purpose))
}

/// What this agent calls itself on every request: its product name and its version. The
/// service keeps the version of each connected agent and shows it beside its own in the update
/// view, with a hint when the agent is older (RD-190-07) -- the case where the agent did not
/// pick up its replaced program file by itself (`relaunch`).
#[must_use]
pub(crate) fn user_agent() -> String {
    format!(
        "{}/{}",
        rd_core::CAPTURE_AGENT_PRODUCT,
        env!("CARGO_PKG_VERSION")
    )
}

/// Applies one set of deadlines. Separate from [`build`] so a test can drive a short budget
/// without waiting out the real one.
fn build_with(deadlines: Deadlines) -> Result<Client> {
    let mut builder = Client::builder()
        .no_proxy()
        .user_agent(user_agent())
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
pub(crate) struct ServiceRefusal {
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
pub(crate) enum Detail {
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
    pub(crate) fn new(operation: &str, status: StatusCode, body: &str) -> Self {
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
    pub(crate) fn unreadable(operation: &str, status: StatusCode, cause: &str) -> Self {
        Self {
            operation: operation.to_owned(),
            status,
            code: None,
            detail: Detail::Unreadable(cause.chars().take(1024).collect()),
        }
    }

    /// The stable `code` the service sent, when the body carried one.
    pub(crate) fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// The HTTP status the service answered with.
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    /// What became of the body.
    pub(crate) fn detail(&self) -> &Detail {
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

/// The code with which the intake says that the links named a page whose releases wait for a
/// choice in the LinkGrabber (RD-1170-03).
const PICK_WAITING: &str = "site_rules.pick_waiting";

/// What the service made of links handed to it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Submitted {
    /// They are in the LinkGrabber.
    Added,
    /// They named a page whose releases -- this many -- wait for a choice in the LinkGrabber
    /// (RD-1190-17). A success: the list is on the board, and repeating the hand-over only
    /// lists the page again.
    PickWaiting(u32),
}

impl Submitted {
    /// The notification a page waiting for a choice gets; `None` for links that were added,
    /// which the intake event announces.
    pub(crate) fn notice(self) -> Option<String> {
        match self {
            Self::Added => None,
            Self::PickWaiting(1) => {
                Some("A page lists 1 release; choose it in rDownloader's LinkGrabber".to_owned())
            }
            Self::PickWaiting(count) => Some(format!(
                "A page lists {count} releases; choose them in rDownloader's LinkGrabber"
            )),
        }
    }
}

/// How many releases wait for a choice, when the refusal is the intake's `pick_waiting`
/// (RD-1190-17). The status must be a client error and the code exactly that one; the count
/// is read from the answer's `params` and is 0 when it cannot be.
pub(crate) fn pick_waiting(error: &anyhow::Error) -> Option<u32> {
    let refusal = error.downcast_ref::<ServiceRefusal>()?;
    if !refusal.status().is_client_error() || refusal.code() != Some(PICK_WAITING) {
        return None;
    }
    let Detail::Text(body) = refusal.detail() else {
        return Some(0);
    };
    let entries = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|body| body.get("params")?.get("entries")?.as_str()?.parse().ok());
    Some(entries.unwrap_or(0))
}

#[derive(Clone)]
pub(crate) struct CaptureClient {
    service: Url,
    token: String,
    /// The short calls, with a total deadline.
    http: Client,
    /// The event stream, which has none. Two clients rather than one, because the two kinds of
    /// call want opposite policies and a single client could only ever be wrong for one of them.
    stream: Client,
    /// Whether the service address answered as rDownloader since the last lost connection.
    identity: Identity,
    /// The agent's own update report and what the service answers to it (RD-1210-03).
    self_update: crate::self_update::Shared,
}

impl CaptureClient {
    pub(crate) fn new(service: Url, token: String) -> Result<Self> {
        Ok(Self {
            service,
            token,
            http: build(Purpose::Request)?,
            stream: build(Purpose::Stream)?,
            identity: Identity::default(),
            self_update: crate::self_update::Shared::default(),
        })
    }

    /// A client whose service counts as confirmed, for the tests whose scripted service answers
    /// only the request under test.
    #[cfg(test)]
    pub(crate) fn confirmed(service: Url, token: String) -> Result<Self> {
        let client = Self::new(service, token)?;
        client.identity.assume_confirmed();
        Ok(client)
    }

    /// Sends `request` with the capture token — once the service address has answered as
    /// rDownloader ([`identity`], RD-1200-03). Every request that carries the token goes through
    /// here, so none reaches a listener that did not.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        self.identity.confirm(&self.http, &self.service).await?;
        let sent = request.bearer_auth(&self.token).send().await;
        // Whoever listens there once the service is back is asked again.
        if sent.as_ref().is_err_and(reqwest::Error::is_connect) {
            self.identity.forget();
        }
        Ok(sent?)
    }

    pub(crate) async fn submit_links(
        &self,
        urls: Vec<Url>,
        source: &str,
        package_name: Option<&str>,
        password: Option<&str>,
    ) -> Result<Submitted> {
        let source = match source {
            "clipboard" => IngressSource::Clipboard,
            "click_and_load" => IngressSource::ClickAndLoad,
            _ => IngressSource::Api,
        };
        let endpoint = self.service.join("api/v1/capture/batches")?;
        let body = serde_json::json!({
            "text": urls.iter().map(Url::as_str).collect::<Vec<_>>().join("\n"),
            "source": source,
            "source_label": "Capture-Agent",
            "package_name": package_name,
            "password": password
        });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        match ensure_success(response, "collector").await {
            Ok(_) => Ok(Submitted::Added),
            Err(error) => pick_waiting(&error)
                .map(Submitted::PickWaiting)
                .ok_or(error),
        }
    }

    /// Figures for the tray: how much is running, and how far along.
    ///
    /// Counts, byte totals and the queue's rate — the capture token is a narrow credential, and
    /// the service answers this one accordingly. Nothing here names a file, a folder or an
    /// account.
    pub(crate) async fn summary(&self) -> anyhow::Result<crate::activity::Summary> {
        let url = self.service.join("api/v1/capture/summary")?;
        let response = self.send(self.http.get(url)).await?;
        let response = ensure_success(response, "transfer summary").await?;
        Ok(response.json().await?)
    }

    /// Pauses the whole queue for the tray (RD-1100-06): for `minutes`, or until resumed.
    ///
    /// Refused with `auth.scope_insufficient` unless the agent was paired with queue control;
    /// the summary says which, so the tray only offers what the service will do.
    pub(crate) async fn pause_queue(&self, minutes: Option<u32>) -> Result<()> {
        let endpoint = self.service.join("api/v1/capture/queue/pause")?;
        let body = serde_json::json!({ "minutes": minutes });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        ensure_success(response, "queue pause").await?;
        Ok(())
    }

    /// Resumes what a pause stopped: ends a timed pause, or queues the paused files again.
    pub(crate) async fn resume_queue(&self) -> Result<()> {
        let endpoint = self.service.join("api/v1/capture/queue/resume")?;
        let body = serde_json::json!({});
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        ensure_success(response, "queue resume").await?;
        Ok(())
    }

    /// What the service has this agent set to: the clipboard pause and the shortcuts
    /// (RD-1180-01, RD-1180-03). Read on the same five-second cadence as the summary.
    /// It carries the agent's own update report and reads the service's channel (RD-1210-03).
    pub(crate) async fn agent_settings(&self) -> Result<rd_core::CaptureAgentSettings> {
        let url = self.service.join("api/v1/capture/agent-settings")?;
        let response = self.send(self.with_report(self.http.get(url))).await?;
        let response = ensure_success(response, "agent settings").await?;
        self.self_update.learn(response.headers());
        Ok(response.json().await?)
    }

    /// Pauses or resumes clipboard watching at the service, which keeps it for every start.
    pub(crate) async fn set_clipboard_paused(
        &self,
        paused: bool,
    ) -> Result<rd_core::CaptureAgentSettings> {
        let endpoint = self.service.join("api/v1/capture/clipboard")?;
        let body = serde_json::json!({ "paused": paused });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        let response = ensure_success(response, "clipboard pause").await?;
        Ok(response.json().await?)
    }

    /// Tells the service which shortcuts the system refused, so the settings page can say so.
    pub(crate) async fn report_shortcuts(
        &self,
        report: &rd_core::CaptureShortcutReport,
    ) -> Result<()> {
        let endpoint = self.service.join("api/v1/capture/shortcut-report")?;
        let response = self.send(self.http.post(endpoint).json(report)).await?;
        ensure_success(response, "shortcut report").await?;
        Ok(())
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
    pub(crate) async fn capture_events(
        &self,
        last_event_id: Option<&str>,
    ) -> Result<reqwest::Response> {
        let endpoint = self.service.join("api/v1/capture/events")?;
        let mut request = self
            .stream
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("Last-Event-ID", id);
        }
        let response = self.send(request).await?;
        ensure_success(response, "event stream").await
    }

    pub(crate) async fn upload_nzb_bytes(&self, name: String, content: Vec<u8>) -> Result<()> {
        let form = multipart::Form::new().part(
            "file",
            multipart::Part::bytes(content)
                .file_name(name)
                .mime_str("application/x-nzb")?,
        );
        let endpoint = self.service.join("api/v1/capture/nzb")?;
        let response = self.send(self.http.post(endpoint).multipart(form)).await?;
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
#[path = "client_tests.rs"]
mod tests;

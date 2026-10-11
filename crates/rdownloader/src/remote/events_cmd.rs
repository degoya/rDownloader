//! `rdownloader events` (RD-1240-18): follows the service's event stream, `GET /api/v1/events`,
//! one line per event until the reader stops it.
//!
//! What a credential sees is what the web interface's stream shows it: a read token gets the
//! queue and the progress, a full one the whole bus. A dropped connection is resumed with the
//! last id seen (`Last-Event-ID`, RD-110-23) after the interval the service's `retry:` names,
//! so a restart of the service costs a follower nothing it could have been handed again; what
//! the service no longer holds is announced by its `stream.expired` marker, printed like any
//! other event. A refusal — a wrong token, a server that never answered at all — ends the
//! command with its exit code instead of retrying something that cannot succeed.

use std::{io::Write, time::Duration};

use anyhow::Result;
use serde_json::Value;

use super::{
    Client, CommandError, Failure, Format,
    output::shorten,
    sse::{Frame, Parser},
};

/// The route the web interface follows too.
pub(super) const EVENTS_PATH: &str = "/api/v1/events";

/// The interval until the service names its own: the `RECONNECT_AFTER` it sends.
const FIRST_RETRY: Duration = Duration::from_secs(5);

/// The bounds a `retry:` is held to: never a reconnect loop without a pause, never longer
/// than a minute.
const MIN_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(60);

/// How much of a payload one text line shows.
const PAYLOAD_WIDTH: usize = 160;

/// Why one connection ended without an error.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Ended {
    /// The service closed the stream: it stopped, or the credential stopped standing.
    Server,
    /// Standard output was closed, as `| head` does.
    Reader,
}

/// Where a follower stands between connections.
pub(super) struct Follower {
    format: Format,
    /// The id of the last event seen, sent back on the next connection.
    last_id: Option<String>,
    /// How long to wait before connecting again.
    retry: Duration,
    connected: bool,
}

impl Follower {
    pub(super) fn new(format: Format) -> Self {
        Self {
            format,
            last_id: None,
            retry: FIRST_RETRY,
            connected: false,
        }
    }

    /// Follows the stream until the reader stops or the server refuses.
    pub(super) async fn run(&mut self, client: &Client, out: &mut impl Write) -> Result<()> {
        loop {
            match self.connection(client, out).await {
                Ok(Ended::Reader) => return Ok(()),
                Ok(Ended::Server) => eprintln!(
                    "The event stream ended; reconnecting in {} s.",
                    self.retry.as_secs()
                ),
                Err(error) => {
                    // Only a connection that worked before is worth waiting for: a first
                    // attempt that fails is a wrong address, and a refusal stays a refusal.
                    let lost = self.connected
                        && error
                            .downcast_ref::<CommandError>()
                            .is_some_and(|command| command.failure == Failure::Unreachable);
                    if !lost {
                        return Err(error);
                    }
                    eprintln!("{error}; reconnecting in {} s.", self.retry.as_secs());
                }
            }
            tokio::time::sleep(self.retry).await;
        }
    }

    /// One connection, from the request to the end of its body.
    pub(super) async fn connection(
        &mut self,
        client: &Client,
        out: &mut impl Write,
    ) -> Result<Ended> {
        let mut response = client
            .open_stream(EVENTS_PATH, self.last_id.as_deref())
            .await?;
        if !self.connected {
            // On standard error, so it never mixes with the lines a script reads.
            eprintln!("Following events from {} (Ctrl+C to stop).", client.base());
        }
        self.connected = true;
        let mut parser = Parser::default();
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => return Ok(Ended::Server),
                Err(error) => {
                    return Err(CommandError::new(
                        Failure::Unreachable,
                        format!("the event stream from {} broke off: {error}", client.base()),
                    )
                    .into());
                }
            };
            for frame in parser.feed(&chunk)? {
                if !self.take(frame, out)? {
                    return Ok(Ended::Reader);
                }
            }
        }
    }

    /// Notes a frame's id and `retry:`, and prints its event if it carries one. `false` once
    /// nobody reads the output any more.
    fn take(&mut self, frame: Frame, out: &mut impl Write) -> Result<bool> {
        if let Some(retry) = frame.retry {
            self.retry = retry.clamp(MIN_RETRY, MAX_RETRY);
        }
        if let Some(id) = frame.id {
            self.last_id = Some(id);
        }
        let Some(data) = frame.data else {
            return Ok(true);
        };
        let line = render(self.format, frame.event.as_deref(), &data);
        match writeln!(out, "{line}").and_then(|()| out.flush()) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

/// One event as one line.
///
/// `--json` prints JSON Lines: an event is the server's envelope (`id`, `kind`, `occurred_at`,
/// `payload`) on a line of its own, and a marker — `stream.lagged`, `stream.expired`, which are
/// not bus events — is `{"kind": <its name>, "payload": <its body>}`, so `jq .kind` reads
/// every line. The text form is the time to the second, the event's name and its payload,
/// shortened to what fits a line.
pub(super) fn render(format: Format, event: Option<&str>, data: &str) -> String {
    let value: Value =
        serde_json::from_str(data).unwrap_or_else(|_| Value::String(data.to_owned()));
    let envelope = value.get("kind").is_some();
    match format {
        Format::Json if envelope => value.to_string(),
        Format::Json => serde_json::json!({
            "kind": event.unwrap_or("message"),
            "payload": value,
        })
        .to_string(),
        Format::Text => {
            let (time, payload) = if envelope {
                let time = value
                    .get("occurred_at")
                    .and_then(Value::as_str)
                    .map(to_the_second)
                    .unwrap_or_default();
                (time, value.get("payload").cloned().unwrap_or(Value::Null))
            } else {
                (String::new(), value.clone())
            };
            let name = event
                .map(ToOwned::to_owned)
                .or_else(|| value.get("kind").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| "message".to_owned());
            let payload = match payload {
                Value::String(text) => text,
                other => other.to_string(),
            };
            format!("{time:<20}  {name}  {}", shorten(&payload, PAYLOAD_WIDTH))
                .trim_end()
                .to_owned()
        }
    }
}

/// `2026-10-10T08:15:02.123456789Z` as `2026-10-10T08:15:02Z`; anything else unchanged.
fn to_the_second(at: &str) -> String {
    match at.split_once('.') {
        Some((second, fraction))
            if fraction
                .strip_suffix('Z')
                .is_some_and(|digits| digits.bytes().all(|byte| byte.is_ascii_digit())) =>
        {
            format!("{second}Z")
        }
        _ => at.to_owned(),
    }
}

#[cfg(test)]
#[path = "events_cmd_tests.rs"]
mod tests;

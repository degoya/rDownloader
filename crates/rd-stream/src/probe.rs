//! Liveness probe: `streamlink --json <url>` reports the available streams of a page.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use rd_tools::process::run_to_output;

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Whether the probe output announces at least one playable stream.
///
/// streamlink prints a JSON object with a non-empty `streams` map while live and an
/// `error` field (exit status 1) while offline; both cases parse, everything else is
/// treated as "not live".
#[must_use]
pub fn parse_probe_output(output: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(output.trim())
        .ok()
        .and_then(|value| {
            Some(
                value.get("error").is_none()
                    && value
                        .get("streams")?
                        .as_object()
                        .is_some_and(|streams| !streams.is_empty()),
            )
        })
        .unwrap_or(false)
}

/// What one probe learned about a channel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamProbe {
    pub live: bool,
    /// Whether the provider offers the stream from its beginning (RD-080-08).
    ///
    /// Detected, never assumed: replay is a provider feature, and offering it in the UI
    /// where it does not exist would promise a recording that silently starts at "now".
    pub replay_available: bool,
}

/// Whether the probe output shows a stream that can be played from its start.
///
/// streamlink exposes this as a `--hls-start-offset`-capable stream, which in the JSON shows
/// up as an HLS stream whose playlist is not a live edge — the plugin reports the DVR window
/// through `start_offset`/`duration` on the stream entry. Absent means no, which is the safe
/// direction: claiming a capability the provider does not have would produce a recording
/// that quietly begins in the middle.
#[must_use]
pub fn parse_replay_capability(output: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output.trim()) else {
        return false;
    };
    let Some(streams) = value.get("streams").and_then(serde_json::Value::as_object) else {
        return false;
    };
    streams.values().any(|stream| {
        // A finite duration, or an explicit start offset, means the provider is serving a
        // window rather than only the live edge.
        stream
            .get("duration")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|duration| duration > 0.0)
            || stream.get("start_offset").is_some()
    })
}

/// Probes one channel URL; `Ok(true)` = live now. Errors mean the probe itself failed
/// (missing tool, timeout), not that the channel is offline.
pub async fn probe_live(streamlink: &Path, url: &str) -> Result<bool> {
    Ok(probe_stream(streamlink, url).await?.live)
}

/// Runs `streamlink --json <url>` and hands back what it printed to stdout.
///
/// Both public probes ask streamlink this one question and differ only in what they read out
/// of the answer, so the invocation is written once. The stdio wiring, the timeout and the
/// Windows console-window flag come from rd-tools, which owns them for every short tool run.
///
/// **The exit status is deliberately not checked.** streamlink exits 1 and prints a JSON
/// `error` field when a channel is simply offline, which is an answer and not a failure; the
/// parsers below decide what the text means. An `Err` here is the probe itself failing —
/// a missing binary or a timeout — and that is what the callers must not read as "offline".
async fn probe_output(streamlink: &Path, url: &str) -> Result<String> {
    let mut command = tokio::process::Command::new(streamlink);
    command.args(["--json", "--"]).arg(url);
    let output = run_to_output(&mut command, PROBE_TIMEOUT)
        .await
        .context("streamlink probe timed out")?
        .context("spawn streamlink probe")?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The raw `streamlink --json` output, for the sidecar capture (RD-080-09).
pub async fn probe_json(streamlink: &Path, url: &str) -> Result<String> {
    probe_output(streamlink, url).await
}

/// Probes one channel URL and reports both liveness and replay capability.
pub async fn probe_stream(streamlink: &Path, url: &str) -> Result<StreamProbe> {
    let text = probe_output(streamlink, url).await?;
    Ok(StreamProbe {
        live: parse_probe_output(&text),
        replay_available: parse_replay_capability(&text),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_probe_output, parse_replay_capability};

    #[test]
    fn detects_live_offline_and_garbage() {
        assert!(parse_probe_output(
            r#"{"plugin":"twitch","metadata":{},"streams":{"best":{"type":"hls"},"720p":{"type":"hls"}}}"#
        ));
        assert!(!parse_probe_output(
            r#"{"error":"No playable streams found on this URL: https://twitch.tv/x"}"#
        ));
        assert!(!parse_probe_output(r#"{"plugin":"twitch","streams":{}}"#));
        assert!(!parse_probe_output("not json"));
        assert!(!parse_probe_output(""));
    }

    #[test]
    fn replay_is_detected_only_when_the_provider_offers_a_window() {
        // A DVR window shows up as a finite duration or an explicit start offset.
        assert!(parse_replay_capability(
            r#"{"streams":{"best":{"type":"hls","duration":7200.0}}}"#
        ));
        assert!(parse_replay_capability(
            r#"{"streams":{"best":{"type":"hls","start_offset":0}}}"#
        ));
    }

    #[test]
    fn a_live_edge_only_stream_reports_no_replay() {
        // The important direction. Claiming replay where there is none produces a recording
        // that silently starts at "now" while the UI says it started at the beginning.
        assert!(!parse_replay_capability(
            r#"{"streams":{"best":{"type":"hls"},"720p":{"type":"hls"}}}"#
        ));
        assert!(!parse_replay_capability(
            r#"{"streams":{"best":{"type":"hls","duration":0}}}"#
        ));
        assert!(!parse_replay_capability(
            r#"{"error":"No playable streams found on this URL"}"#
        ));
        assert!(!parse_replay_capability("not json"));
        assert!(!parse_replay_capability(""));
    }
}

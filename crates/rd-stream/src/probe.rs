//! Liveness probe: `streamlink --json <url>` reports the available streams of a page.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use rd_scheduler::ToolNetwork;
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
/// (missing tool, timeout, a proxy streamlink cannot use), not that the channel is offline.
pub async fn probe_live(streamlink: &Path, url: &str, network: &ToolNetwork) -> Result<bool> {
    Ok(probe_stream(streamlink, url, network).await?.live)
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
async fn probe_output(streamlink: &Path, url: &str, network: &ToolNetwork) -> Result<String> {
    let mut command = crate::segments::streamlink_command(streamlink, network);
    command.args(["--json", "--"]).arg(url);
    let output = run_to_output(&mut command, PROBE_TIMEOUT)
        .await
        .context("streamlink probe timed out")?
        .context("spawn streamlink probe")?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The raw `streamlink --json` output, for the sidecar capture (RD-080-09), through the
/// recording's proxy (RD-1240-08).
pub(crate) async fn probe_json(
    streamlink: &Path,
    url: &str,
    network: &ToolNetwork,
) -> Result<String> {
    probe_output(streamlink, url, network).await
}

/// Probes one channel URL through `network` and reports both liveness and replay capability.
///
/// The channel monitor hands in the global proxy profile (RD-1240-22). A streamlink that cannot
/// speak that proxy says so in the `error` field an offline channel uses too; read as
/// "offline", the channel would never be recorded and nobody would learn why. A proxy that
/// refuses the profile's password is the same silence (RD-1240-29).
pub async fn probe_stream(
    streamlink: &Path,
    url: &str,
    network: &ToolNetwork,
) -> Result<StreamProbe> {
    let text = probe_output(streamlink, url, network).await?;
    if let Some(failure) = probe_error(&text).and_then(|error| {
        network
            .unsupported_proxy("streamlink", &error)
            .or_else(|| rd_scheduler::proxy_auth_failed(&error))
    }) {
        anyhow::bail!(failure.message);
    }
    Ok(StreamProbe {
        live: parse_probe_output(&text),
        replay_available: parse_replay_capability(&text),
    })
}

/// The `error` field of streamlink's JSON answer, when it has one.
fn probe_error(output: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(output.trim())
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
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

/// RD-1240-22 — the channel monitor's probe goes through the proxy it is handed.
#[cfg(all(test, unix))]
mod network_tests {
    use std::{
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use rd_core::ProxyKind;
    use rd_scheduler::{ToolNetwork, ToolProxy};

    use super::probe_stream;

    const CHANNEL: &str = "https://live.example/channel";

    /// A streamlink that writes its argument list to `args.log` beside it and prints `answer`.
    fn fake_streamlink(directory: &Path, answer: &str) -> PathBuf {
        let script = directory.join("streamlink");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nprintf '%s' '{answer}'\n",
                directory.join("args.log").display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        script
    }

    fn proxied(kind: ProxyKind, endpoint: &str) -> ToolNetwork {
        ToolNetwork::with_proxy(
            ToolProxy::new(kind, endpoint.parse().expect("endpoint"), None, None).expect("proxy"),
        )
    }

    #[tokio::test]
    async fn the_probe_carries_the_proxy_it_is_handed() {
        let directory = tempfile::tempdir().expect("directory");
        let streamlink = fake_streamlink(
            directory.path(),
            r#"{"plugin":"twitch","streams":{"best":{"type":"hls"}}}"#,
        );
        let network = proxied(ProxyKind::Http, "http://proxy.example:3128");

        let probe = probe_stream(&streamlink, CHANNEL, &network)
            .await
            .expect("probe");
        assert!(probe.live);
        let args = std::fs::read_to_string(directory.path().join("args.log")).expect("args");
        assert!(
            args.starts_with("--http-proxy http://proxy.example:3128/ --json -- "),
            "{args}"
        );
    }

    /// A streamlink that cannot speak the proxy answers in the field an offline channel uses;
    /// through a proxy that is a failure of the probe, never "offline".
    #[tokio::test]
    async fn a_proxy_streamlink_cannot_use_is_not_taken_for_offline() {
        let directory = tempfile::tempdir().expect("directory");
        let streamlink = fake_streamlink(
            directory.path(),
            r#"{"error":"Unable to open URL: https://live.example/channel (Missing dependencies for SOCKS support.)"}"#,
        );

        let network = proxied(ProxyKind::Socks5, "socks5h://proxy.example:1080");
        let error = probe_stream(&streamlink, CHANNEL, &network)
            .await
            .expect_err("no answer through the proxy");
        assert!(error.to_string().contains("SOCKS5"), "{error}");

        // Without a proxy the same answer stays what it was: not live.
        let probe = probe_stream(&streamlink, CHANNEL, &ToolNetwork::direct())
            .await
            .expect("probe");
        assert!(!probe.live);
    }

    /// RD-1240-29: a proxy that refuses the profile's password answers in the same field; it is
    /// the proxy's failure, never "offline". (The real line quotes `ProxyError('…')`, which the
    /// fake's single-quoted `printf` cannot carry.)
    #[tokio::test]
    async fn a_proxy_refusing_its_password_is_not_taken_for_offline() {
        let directory = tempfile::tempdir().expect("directory");
        let streamlink = fake_streamlink(
            directory.path(),
            r#"{"error":"Unable to open URL: https://live.example/channel (Tunnel connection failed: 407 Proxy Authentication Required)"}"#,
        );

        let network = proxied(ProxyKind::Http, "http://proxy.example:3128");
        let error = probe_stream(&streamlink, CHANNEL, &network)
            .await
            .expect_err("refused by the proxy");
        assert!(error.to_string().contains("407"), "{error}");
    }
}

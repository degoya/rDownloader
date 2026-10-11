//! Queue runner: records one livestream with `streamlink --output` until the stream ends
//! or the user stops it. A stop with recorded bytes finalizes the file as completed, so
//! post-processing (including rclone upload) still runs.

use std::{ops::ControlFlow, path::PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{DownloadFile, DownloadKind, DownloadPackage, Failure, FailureKind};
use rd_db::Database;
use rd_http::SharedNetworkDefaults;
use rd_scheduler::{ExternalRunner, RunOutcome, ToolNetwork, ToolNetworkSource};
use tokio_util::sync::CancellationToken;

use crate::{SharedStreamSettings, sidecars::SidecarClients};

/// Records `DownloadKind::Record` files.
pub struct StreamRunner {
    database: Database,
    settings: SharedStreamSettings,
    /// Read on every dispatch pass, so a changed setting needs no restart (audit 1.9.1, TR-07).
    slots: rd_tools::LiveSlots<rd_core::StreamSettings>,
    /// The HTTP client the sidecar fetches use. Default until
    /// [`Self::with_network_defaults`] hands over the scheduler's, which is the platform
    /// store alone - the behaviour of an installation without a custom CA.
    sidecars: SidecarClients,
    /// The proxy and CA streamlink is started with (RD-1240-08); none without it, as in tests.
    network: Option<ToolNetworkSource>,
}

impl StreamRunner {
    #[must_use]
    pub fn new(database: Database, settings: SharedStreamSettings) -> Self {
        let slots = rd_tools::LiveSlots::new(std::sync::Arc::clone(&settings), |settings| {
            settings.record_max_parallel
        });
        Self {
            database,
            settings,
            slots,
            sidecars: SidecarClients::new(),
            network: None,
        }
    }

    /// Hands streamlink the recording's proxy and the custom CA (RD-1240-08).
    #[must_use]
    pub fn with_tool_network(mut self, network: ToolNetworkSource) -> Self {
        self.network = Some(network);
        self
    }

    /// Connects the sidecar fetches to the shared network defaults, which is where the
    /// operator's custom CA lives.
    ///
    /// Without this a thumbnail is fetched against the platform trust store alone, so a
    /// provider behind an internal CA reports the thumbnail as failed while the recording
    /// itself succeeds - the same inconsistency `with_network_defaults` closed for NNTP.
    #[must_use]
    pub fn with_network_defaults(mut self, network: SharedNetworkDefaults) -> Self {
        self.sidecars = SidecarClients::with_network_defaults(network);
        self
    }
}

/// Maps a streamlink failure (no bytes recorded) onto the retry policy.
pub(crate) fn map_stream_error(stderr: &str) -> Failure {
    // A proxy refusing the profile's password was a failure retried every two minutes, never
    // the proxy's (RD-1240-29).
    if let Some(failure) = rd_scheduler::proxy_auth_failed(stderr) {
        return failure;
    }
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("no plugin can handle url") {
        return Failure::coded(
            FailureKind::Unsupported,
            "record.unsupported_url",
            "streamlink has no plugin for this URL",
        );
    }
    if lower.contains("no playable streams") {
        // Offline channel: retrying doubles as a lightweight "wait until live".
        return Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: Some(300),
            },
            "record.not_live",
            "The channel is not live",
        );
    }
    rd_tools::tool_failed("record.tool_failed", stderr, "streamlink failed")
}

impl StreamRunner {
    /// The recording policy for one file: the channel's, matched by URL.
    ///
    /// Matched by address rather than carried on the queue row, so a policy edited while a
    /// recording is queued applies to it — and so a recording started by "record now", which
    /// has no channel row to point at, still picks up that channel's settings.
    async fn policy_for(&self, file: &DownloadFile) -> rd_core::RecordingPolicy {
        self.database
            .list_stream_channels()
            .await
            .ok()
            .and_then(|channels| {
                channels
                    .into_iter()
                    .find(|channel| channel.url == file.source.as_str())
                    .map(|channel| channel.recording)
            })
            .filter(rd_core::RecordingPolicy::is_valid)
            .unwrap_or_default()
    }
}

#[async_trait]
impl ExternalRunner for StreamRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Record
    }

    /// A live recording cannot be resumed or re-fetched: what was missed is gone.
    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: false,
            recheck_partial: false,
            adopt_completed: false,
            verify_completed: false,
            applies_collision_policy: false,
        }
    }

    fn slot_capacity(&self) -> usize {
        self.slots.get()
    }

    /// A recording can run for hours; it must never occupy a regular download slot.
    fn counts_against_global_limit(&self) -> bool {
        false
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        _limits: rd_scheduler::RunLimits,
    ) -> Result<RunOutcome> {
        let recording = match self.prepare_recording(file, package).await? {
            ControlFlow::Continue(recording) => recording,
            ControlFlow::Break(outcome) => return Ok(outcome),
        };
        let (state, last_error) = match self
            .record_segments(file, &recording, &cancellation)
            .await?
        {
            ControlFlow::Continue(recorded) => recorded,
            ControlFlow::Break(outcome) => return Ok(outcome),
        };
        Ok(self.conclude(file, &recording, state, &last_error).await)
    }
}

/// What one recording works with, settled before its first segment.
struct Recording {
    /// Held for the whole recording: its lease keeps the binary on disk.
    streamlink: rd_tools::PreparedTool,
    quality: String,
    directory: PathBuf,
    policy: rd_core::RecordingPolicy,
    stem: String,
    /// The proxy and CA every segment and the sidecar probe go through (RD-1240-08).
    network: ToolNetwork,
}

impl StreamRunner {
    /// Leases Streamlink and settles quality, folder, policy and name, or names the outcome
    /// that ends the run first.
    async fn prepare_recording(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
    ) -> Result<ControlFlow<RunOutcome, Recording>> {
        // A proxy that cannot be used is a failure, never a direct connection.
        let network = match &self.network {
            Some(source) => match source.for_file(file).await {
                Ok(network) => network,
                Err(failure) => return Ok(ControlFlow::Break(RunOutcome::Failed(failure))),
            },
            None => ToolNetwork::direct(),
        };
        let settings = self.settings.read().await.clone();
        // Leased before the version is assessed, and only recordings stop when Streamlink is
        // too old or listed as broken (RD-102-02, RD-102-03); both rules live in `prepare`.
        // The lookup stays here because a recorder also accepts the portable Windows layout,
        // which `locate_tool_leased` does not know about.
        let streamlink = match rd_tools::process::prepare(
            "streamlink",
            crate::lease_streamlink(&settings).map(|(tool, lease)| (tool.path, lease)),
            rd_tools::Capability::StreamRecording,
        )
        .await
        {
            Ok(tool) => tool,
            Err(failure) => return Ok(ControlFlow::Break(RunOutcome::Failed(failure))),
        };
        let quality = file
            .media
            .as_ref()
            .map(|selection| selection.format.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| settings.record_default_quality.clone());
        let directory = PathBuf::from(&package.destination);
        tokio::fs::create_dir_all(&directory).await?;

        let policy = self.policy_for(file).await;
        let stem = std::path::Path::new(&file.file_name)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("recording")
            .to_owned();
        Ok(ControlFlow::Continue(Recording {
            streamlink,
            quality,
            directory,
            policy,
            stem,
            network,
        }))
    }

    /// Records segment after segment until the stream ends or the user stops it. Hands back
    /// the recorded state and the last error, or the outcome that ends the run without one.
    async fn record_segments(
        &self,
        file: &DownloadFile,
        recording: &Recording,
        cancellation: &CancellationToken,
    ) -> Result<ControlFlow<RunOutcome, (rd_core::RecordingState, String)>> {
        let policy = recording.policy;
        // Recorded segments accumulate here and are persisted after each one, so a crash
        // mid-recording leaves a history that says what is on disk rather than nothing.
        let mut state = rd_core::RecordingState::default();
        let mut index: u32 = 0;
        let mut last_error = String::new();

        loop {
            index += 1;
            let name = crate::segments::next_name(&recording.stem, index);
            let output = recording.directory.join(&name);
            let started_at = chrono::Utc::now();
            let carried = state.total_bytes();

            let (outcome, bytes, stderr_text) = self
                .record_segment(file, recording, &output, carried, cancellation)
                .await?;

            if bytes > 0 {
                crate::segments::push_segment(
                    &mut state,
                    index,
                    name.clone(),
                    bytes,
                    started_at,
                    outcome,
                );
                let _ = self
                    .database
                    .set_download_recording_state(file.id, state.clone())
                    .await;
            } else {
                // An empty segment leaves no file behind to confuse the remux step.
                let _ = tokio::fs::remove_file(&output).await;
                last_error = stderr_text.clone();
            }

            if !crate::segments::should_continue(outcome, state.reconnects, index) {
                if outcome == crate::segments::SegmentOutcome::Cancelled
                    && state.segments.is_empty()
                {
                    return Ok(ControlFlow::Break(RunOutcome::Stopped));
                }
                if state.segments.is_empty() {
                    return Ok(ControlFlow::Break(RunOutcome::Failed(
                        recording
                            .network
                            .unsupported_proxy("streamlink", &last_error)
                            .unwrap_or_else(|| map_stream_error(&last_error)),
                    )));
                }
                break;
            }

            tracing::info!(
                file = %file.file_name,
                segment = index,
                reconnects = state.reconnects,
                "recording continues in a new segment"
            );
            if outcome == crate::segments::SegmentOutcome::Dropped
                && policy.reconnect_delay_seconds > 0
            {
                let delay =
                    std::time::Duration::from_secs(u64::from(policy.reconnect_delay_seconds));
                tokio::select! {
                    () = cancellation.cancelled() => break,
                    () = tokio::time::sleep(delay) => {}
                }
            }
        }
        Ok(ControlFlow::Continue((state, last_error)))
    }

    /// Records one segment into `output`, reporting progress on the way.
    async fn record_segment(
        &self,
        file: &DownloadFile,
        recording: &Recording,
        output: &std::path::Path,
        carried: u64,
        cancellation: &CancellationToken,
    ) -> Result<(crate::segments::SegmentOutcome, u64, String)> {
        let database = self.database.clone();
        let file_id = file.id;
        crate::segments::record(
            &crate::segments::SegmentTool {
                streamlink: recording.streamlink.path(),
                url: file.source.as_str(),
                quality: &recording.quality,
                network: &recording.network,
            },
            output,
            recording.policy,
            carried,
            cancellation,
            move |total| {
                // Fire-and-forget: progress is advisory, and awaiting it here would stall
                // the sampling loop behind the database writer.
                let database = database.clone();
                tokio::spawn(async move {
                    let _ = database.set_download_progress(file_id, total, None).await;
                });
            },
        )
        .await
    }

    /// Captures the sidecars, records the final state and progress and names the recording.
    async fn conclude(
        &self,
        file: &DownloadFile,
        recording: &Recording,
        mut state: rd_core::RecordingState,
        last_error: &str,
    ) -> RunOutcome {
        let policy = recording.policy;
        // Sidecars are captured once, after the recording, so a reconnect does not refetch
        // them and they are unambiguously the recording's own.
        if !policy.sidecars.is_empty() {
            state.sidecars = crate::sidecars::capture(
                &self.sidecars,
                recording.streamlink.path(),
                file.source.as_str(),
                &recording.directory,
                &recording.stem,
                policy.sidecars,
                &recording.network,
            )
            .await;
        }
        let _ = self
            .database
            .set_download_recording_state(file.id, state.clone())
            .await;

        let total = state.total_bytes();
        let _ = self
            .database
            .set_download_progress(file.id, total, Some(total))
            .await;
        if !last_error.trim().is_empty() {
            tracing::warn!(
                file = %file.file_name,
                output = %rd_core::redact_text(
                    &last_error.trim().chars().take(600).collect::<String>()
                ),
                "streamlink reported errors during the recording"
            );
        }
        // The first segment is the recording's name for the queue; the rest sit beside it,
        // and the remux step (RD-080-09) joins them if one was asked for.
        let final_name = state
            .segments
            .first()
            .map(|segment| segment.file_name.clone())
            .unwrap_or_else(|| file.file_name.clone());
        RunOutcome::Completed { final_name }
    }
}

#[cfg(test)]
mod tests {
    use super::map_stream_error;
    use rd_core::FailureKind;

    #[test]
    fn errors_map_to_retry_policy() {
        assert!(matches!(
            map_stream_error("error: No plugin can handle URL: https://x").category,
            FailureKind::Unsupported
        ));
        let offline = map_stream_error("error: No playable streams found on this URL");
        assert!(matches!(
            offline.category,
            FailureKind::Transient {
                retry_after_seconds: Some(300)
            }
        ));
        assert!(matches!(
            map_stream_error("some transport error").category,
            FailureKind::Transient { .. }
        ));
    }

    /// RD-1240-29: streamlink's words for a proxy that refused the profile's password are the
    /// proxy's permanent failure, not a recording retried every two minutes.
    #[test]
    fn a_proxy_refusing_its_password_is_a_proxy_failure() {
        let failure = map_stream_error(
            "[cli][info] Found matching plugin twitch for URL https://www.twitch.tv/example\n\
             error: Unable to open URL: https://gql.twitch.tv/gql (HTTPSConnectionPool(host=\
             'gql.twitch.tv', port=443): Max retries exceeded with url: /gql (Caused by \
             ProxyError('Unable to connect to proxy', OSError('Tunnel connection failed: 407 \
             Proxy Authentication Required'))))",
        );
        assert_eq!(failure.code.as_deref(), Some("proxy.auth_failed"));
        assert_eq!(failure.category, FailureKind::Permanent);
    }
}

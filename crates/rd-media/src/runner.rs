//! Queue runner: downloads one media file with `yt-dlp` into the package folder.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rd_core::{
    AuthProfileSelection, DownloadFile, DownloadKind, DownloadPackage, Failure, FailureKind,
    MediaFormatCriteria, MediaKind, MediaSelection,
};
use rd_db::Database;
use rd_scheduler::{ExternalRunner, RunOutcome};
use rd_secrets::SecretStore;
use rd_tools::{
    LiveSlots, ProgressThrottle, ToolLine, ToolProcess,
    process::{Stdout, prepare},
};
use tokio_util::sync::CancellationToken;

use crate::{
    SharedMediaSettings,
    args::{DownloadPlan, output_mode},
    cookies::{CookieError, CookieFile},
    merge,
    probe::map_tool_error,
    progress::parse_progress_line,
    select::MediaCapabilities,
    tools::{FfmpegTools, lease_tool},
};

#[path = "runner_ytdlp.rs"]
mod ytdlp;

#[cfg(test)]
use ytdlp::progressive_format;
use ytdlp::{
    YTDLP_SUFFIX_RESERVE, final_path_line, output_template, resolve_format, template_values,
};

/// Downloads `DownloadKind::Media` files.
pub struct MediaRunner {
    database: Database,
    secrets: SecretStore,
    settings: SharedMediaSettings,
    /// Read on every dispatch pass, so a changed setting needs no restart (audit 1.9.1, TR-07).
    slots: LiveSlots<rd_core::MediaSettings>,
}

impl MediaRunner {
    #[must_use]
    pub fn new(database: Database, secrets: SecretStore, settings: SharedMediaSettings) -> Self {
        let slots = LiveSlots::new(Arc::clone(&settings), |settings| {
            settings.media_max_parallel
        });
        Self {
            database,
            secrets,
            settings,
            slots,
        }
    }

    /// Resolves the file's cookie profile into a scoped, self-deleting cookie file.
    ///
    /// Returns `Ok(None)` when the download asked for no profile and none matches the page,
    /// which is the ordinary case for a public video. A *pinned* profile that cannot be used
    /// is an error rather than a silent fallback: the user asked for that session
    /// specifically, and quietly downloading the public version of a private page is the
    /// kind of "help" that produces a 30-second trailer named like the full episode.
    async fn cookie_file(
        &self,
        file: &DownloadFile,
        selection: &MediaSelection,
    ) -> Result<Option<CookieFile>, CookieError> {
        let (profile, pinned) = match file.auth_profile {
            AuthProfileSelection::None => return Ok(None),
            AuthProfileSelection::Auto => (
                self.database
                    .match_auth_profile(&selection.page_url)
                    .await
                    .map_err(|_| CookieError::SecretUnavailable)?,
                false,
            ),
            AuthProfileSelection::Pinned(id) => (
                self.database
                    .auth_profile(id)
                    .await
                    .map_err(|_| CookieError::SecretUnavailable)?,
                true,
            ),
        };
        let Some(profile) = profile else {
            return if pinned {
                Err(CookieError::ProfileUnusable)
            } else {
                Ok(None)
            };
        };
        // An automatically matched Basic/Bearer profile is not an error: it simply has
        // nothing yt-dlp can use. Pinning one is a mistake worth reporting.
        if profile.method != rd_core::AuthMethod::Cookies && !pinned {
            return Ok(None);
        }
        let Some(reference) = profile.secret_ref.as_deref() else {
            return if pinned {
                Err(CookieError::SecretUnavailable)
            } else {
                Ok(None)
            };
        };
        let secret = self
            .secrets
            .get(reference)
            .await
            .map_err(|_| CookieError::SecretUnavailable)?;
        match crate::cookies::materialize(&profile, &secret, &selection.page_url, Utc::now()) {
            Ok(cookies) => Ok(Some(cookies)),
            // A profile that merely happens to match and has nothing for this host is not a
            // failure; one the user pinned is.
            Err(CookieError::NoCookiesForHost) if !pinned => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[async_trait]
impl ExternalRunner for MediaRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Media
    }

    /// yt-dlp continues its own `.part` files (`--continue`) but proves nothing about them, and
    /// its output template names the file, not the collision policy.
    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: true,
            recheck_partial: false,
            adopt_completed: false,
            verify_completed: false,
            applies_collision_policy: false,
        }
    }

    fn slot_capacity(&self) -> usize {
        self.slots.get()
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: rd_scheduler::RunLimits,
    ) -> Result<RunOutcome> {
        let Some(selection) = file.media.clone() else {
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Permanent,
                "media.selection_missing",
                "Media download has no variant selection",
            )));
        };
        let settings = self.settings.read().await.clone();
        let ytdlp_tool = match lease_ytdlp(&settings).await {
            Ok(tool) => tool,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        // Bound for the whole run: dropping the `PreparedTool` releases the lease, so it may
        // not be reduced to its path here.
        let ytdlp = ytdlp_tool.path();
        let ffmpeg = FfmpegTools::resolve(&settings);
        if let Some(failure) = tool_failure(&selection, &ffmpeg).await {
            return Ok(RunOutcome::Failed(failure));
        }
        let directory = PathBuf::from(&package.destination);
        tokio::fs::create_dir_all(&directory).await?;
        let stem = media_stem(&directory, &file.file_name);
        let capabilities = ffmpeg.media_capabilities().await;
        let criteria = selection.effective_criteria();
        let template = output_target(&directory, &stem, &selection, criteria.as_ref(), &settings);
        // Bound here, before the spawn, and dropped when `run` returns by any path — the
        // success return, an early `Failed`, and the cancellation branch that kills the
        // child all unwind through this binding, so the file cannot outlive the download.
        let cookies = match self.cookie_file(file, &selection).await {
            Ok(cookies) => cookies,
            Err(error) => return Ok(RunOutcome::Failed(cookie_failure(error))),
        };
        let format = match resolve_format(
            ytdlp,
            &settings,
            &selection,
            criteria.as_ref(),
            capabilities,
        )
        .await
        {
            Ok(format) => format,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        // A borrow, not a clone: `EMPTY` is a `const`, so it needs a binding to live long
        // enough for the plan to hold a reference to it.
        let empty_tracks = rd_core::TrackSelection::EMPTY;
        let embed = embed_policy(criteria.as_ref(), &selection, capabilities);
        let plan = DownloadPlan {
            format: &format,
            output: &template,
            ffmpeg_location: ffmpeg.location(),
            cookies: cookies.as_ref().map(CookieFile::path),
            limit_rate: limits
                .bandwidth
                .binding_limit()
                .map(|binding| binding.bytes_per_second),
            output_mode: &output_mode(criteria.as_ref(), selection.kind == MediaKind::Audio),
            tracks: criteria
                .as_ref()
                .map_or(&empty_tracks, |criteria| &criteria.tracks),
            embed: &embed,
            page_url: selection.page_url.as_str(),
        };
        let mut command = tokio::process::Command::new(ytdlp);
        command.args(plan.build());
        let process = ToolProcess::spawn(&mut command, "yt-dlp", Stdout::Read)?;
        self.follow(process, file, &cancellation).await
    }
}

impl MediaRunner {
    /// Reads yt-dlp's output to its end, reporting progress on the way, then judges what it
    /// left behind.
    async fn follow(
        &self,
        mut process: ToolProcess,
        file: &DownloadFile,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome> {
        let mut final_path: Option<String> = None;
        let mut throttle = ProgressThrottle::default();
        // A merged video is fetched as two streams that each count 0-100 %. Bytes of the
        // finished streams are carried over so the reported progress only ever grows.
        let mut finished_bytes: u64 = 0;
        let mut stream_total: Option<u64> = None;
        let mut stream_committed: u64 = 0;
        loop {
            let line = match process.next_line(cancellation.cancelled()).await? {
                ToolLine::Line(line) => line,
                ToolLine::End => break,
                ToolLine::Stopped => return Ok(RunOutcome::Stopped),
                // No deadline is set today: a download runs as long as its bytes take.
                ToolLine::TimedOut => {
                    return Ok(RunOutcome::Failed(timed_out(&process.stderr().await)));
                }
            };
            if line.trim().starts_with("[download] Destination:") {
                finished_bytes += stream_total.unwrap_or(stream_committed);
                stream_total = None;
                stream_committed = 0;
                continue;
            }
            if let Some(progress) = parse_progress_line(&line) {
                stream_total = progress.total_bytes.or(stream_total);
                if let Some(total_bytes) = stream_total {
                    stream_committed = (total_bytes * u64::from(progress.percent_tenths)) / 1000;
                    if throttle.due() {
                        let _ = self
                            .database
                            .set_download_progress(
                                file.id,
                                finished_bytes + stream_committed,
                                Some(finished_bytes + total_bytes),
                            )
                            .await;
                        throttle.mark();
                    }
                }
                continue;
            }
            if let Some(path) = final_path_line(&line) {
                final_path = Some(path.to_owned());
            }
        }
        self.finish(process, file, final_path).await
    }

    /// The verdict on a yt-dlp run that reached its end: its exit, its warnings, and the file
    /// it reported.
    async fn finish(
        &self,
        mut process: ToolProcess,
        file: &DownloadFile,
        final_path: Option<String>,
    ) -> Result<RunOutcome> {
        let status = process.wait().await?;
        let stderr_text = process.stderr().await;
        if !status.success() {
            return Ok(RunOutcome::Failed(map_tool_error(&stderr_text)));
        }
        // yt-dlp reports a missing ffmpeg, an unmergeable format or a skipped post-processing
        // step as a warning and still exits successfully. Silently dropping those is what let
        // "video and audio downloaded separately" go unnoticed.
        if !stderr_text.trim().is_empty() {
            // Redacted: a warning line can quote the request yt-dlp made, and with a
            // cookie file in play that line is a place a session can surface.
            tracing::warn!(
                file = %file.file_name,
                output = %rd_core::redact_text(
                    &stderr_text.trim().chars().take(600).collect::<String>()
                ),
                "yt-dlp reported warnings"
            );
        }
        // Streams yt-dlp could not merge are not a finished download, whatever the exit code.
        if merge::merge_skipped(&stderr_text) {
            return Ok(RunOutcome::Failed(merge::merge_failure()));
        }
        let Some(path) = final_path.map(PathBuf::from) else {
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: Some(120),
                },
                "media.output_missing",
                "yt-dlp finished without reporting the output file",
            )));
        };
        if merge::left_unmerged(&path).await {
            return Ok(RunOutcome::Failed(merge::merge_failure()));
        }
        let final_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("output file name is not UTF-8")?
            .to_owned();
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            let _ = self
                .database
                .set_download_progress(file.id, meta.len(), Some(meta.len()))
                .await;
        }
        Ok(RunOutcome::Completed { final_name })
    }
}

/// The yt-dlp this run uses, leased for as long as the returned tool is held.
async fn lease_ytdlp(settings: &rd_core::MediaSettings) -> Result<rd_tools::PreparedTool, Failure> {
    // Leased, not just located, and leased *before* the version is assessed: a managed
    // yt-dlp stays on disk for as long as this download runs even if another version is
    // activated meanwhile (RD-102-02). A yt-dlp this build does not support cannot be
    // relied on to produce the file that was selected, so media downloads stop and the
    // rest of the queue is untouched (RD-102-03). Both rules live in `prepare`.
    prepare(
        "yt-dlp",
        lease_tool(
            settings.media_ytdlp_executable.as_deref(),
            settings.vendor_directory.as_deref(),
            "yt-dlp",
        ),
        rd_tools::Capability::MediaDownload,
    )
    .await
}

/// The sanitised stem the downloaded file is written under.
fn media_stem(directory: &std::path::Path, file_name: &str) -> String {
    // yt-dlp writes intermediate files next to the target (`<stem>.f137.mp4.part`), so the
    // stem has to leave room for the longest suffix it appends, not just for `.%(ext)s`.
    rd_files::sanitize_file_name_within(
        directory,
        std::path::Path::new(file_name)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("media"),
        YTDLP_SUFFIX_RESERVE,
    )
}

/// A cookie profile that cannot be used, as the download's failure.
fn cookie_failure(error: CookieError) -> Failure {
    Failure::coded(FailureKind::Permanent, error.code(), error.message())
}

/// Why the tools at hand cannot produce this selection, or `None` when they can.
async fn tool_failure(selection: &MediaSelection, ffmpeg: &FfmpegTools) -> Option<Failure> {
    // MP3 conversion cannot happen without the tools, so fail before downloading. A video
    // still succeeds: yt-dlp then falls back to a progressive format instead of merging.
    if selection.kind == MediaKind::Audio
        && let Some(blocking) = ffmpeg.blocking(rd_tools::Capability::AudioExtraction).await
    {
        return Some(rd_tools::compat::incompatible_failure(
            &blocking,
            rd_tools::Capability::AudioExtraction,
        ));
    }
    if selection.kind == MediaKind::Audio && !ffmpeg.is_complete() {
        let missing = if ffmpeg.ffmpeg.is_none() {
            "ffmpeg"
        } else {
            "ffprobe"
        };
        return Some(
            Failure::coded(
                FailureKind::Unsupported,
                "media.tool_missing",
                format!("{missing} is required for MP3 conversion but is not installed"),
            )
            .with_param("tool", missing),
        );
    }
    if selection.kind == MediaKind::Video && !ffmpeg.is_complete() {
        tracing::warn!(
            ffmpeg = ffmpeg.ffmpeg.is_some(),
            ffprobe = ffmpeg.ffprobe.is_some(),
            "ffmpeg and ffprobe are both required to merge video and audio; \
             yt-dlp will fall back to a single pre-muxed stream"
        );
    }
    None
}

/// The `-o` argument: the per-job template or the configured default, expanded in
/// `directory`.
fn output_target(
    directory: &std::path::Path,
    stem: &str,
    selection: &MediaSelection,
    criteria: Option<&MediaFormatCriteria>,
    settings: &rd_core::MediaSettings,
) -> PathBuf {
    // A per-job template beats the configured default; neither is required.
    let output_pattern = criteria
        .and_then(|criteria| criteria.output_template.clone())
        .or_else(|| settings.media_output_template.clone());
    output_template(
        directory,
        stem,
        output_pattern.as_deref(),
        &template_values(selection, stem),
    )
}

/// The embed policy the download's arguments are built with.
fn embed_policy(
    criteria: Option<&MediaFormatCriteria>,
    selection: &MediaSelection,
    capabilities: MediaCapabilities,
) -> rd_core::MediaEmbedPolicy {
    // The policy is reduced to what this container and these tools actually allow
    // *before* the arguments are built, so the flags and what the UI previewed agree.
    criteria.map_or_else(rd_core::MediaEmbedPolicy::default, |criteria| {
        rd_core::effective_policy(
            &criteria.embed,
            &selection.ext,
            &selection.page_url,
            capabilities.can_transcode_audio,
        )
    })
}

/// A run its deadline ended: the tool failed, retried like any other yt-dlp failure. Answered
/// as a stop it read as the person's own pause and was never tried again (re-audit 1.9.1,
/// RA-TR-03).
fn timed_out(stderr: &str) -> Failure {
    let tail = rd_tools::stderr_tail(stderr, "yt-dlp ran past its time limit");
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: Some(120),
        },
        "media.ytdlp_failed",
        format!("yt-dlp failed: {tail}"),
    )
    .with_param("detail", tail)
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;

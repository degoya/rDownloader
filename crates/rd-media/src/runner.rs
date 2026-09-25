//! Queue runner: downloads one media file with `yt-dlp` into the package folder.

use std::{path::PathBuf, time::Duration};

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
    ProgressThrottle, ToolProcess,
    process::{Stdout, prepare},
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::{
    SharedMediaSettings,
    args::{DownloadPlan, FINAL_PATH_MARKER, output_mode},
    cookies::{CookieError, CookieFile},
    probe::map_tool_error,
    progress::parse_progress_line,
    select::{MediaCapabilities, resolve},
    tools::{FfmpegTools, lease_tool},
};

/// The finished file's path out of one yt-dlp stdout line, or `None` for every other line.
///
/// yt-dlp is asked to print the path behind [`FINAL_PATH_MARKER`], so the answer is
/// recognisable. The rule used to be "any non-empty line that does not start with `[`",
/// which meant a single unprefixed line from an extractor — a warning, a merge note, a
/// plugin printing to stdout — overwrote the value, and the runner then reported a
/// `final_name` that had never been written to the queue and to post-processing.
fn final_path_line(line: &str) -> Option<&str> {
    let value = line.trim().strip_prefix(FINAL_PATH_MARKER)?.trim();
    (!value.is_empty()).then_some(value)
}

/// Room kept free after the stem for what yt-dlp appends while downloading, e.g.
/// `.f2120404511882385v.mp4.part` for a per-format fragment of a merged video.
const YTDLP_SUFFIX_RESERVE: usize = 40;

/// Drops the `video+audio` alternatives from a yt-dlp format expression, keeping only the
/// last (pre-muxed) alternative so no merge is required.
///
/// `bv*[height<=1080]+ba/b[height<=1080]` becomes `b[height<=1080]`; an expression that
/// offers no such alternative falls back to `b`.
fn progressive_format(format: &str) -> String {
    format
        .split('/')
        .map(str::trim)
        .rfind(|alternative| !alternative.is_empty() && !alternative.contains('+'))
        .unwrap_or("b")
        .to_owned()
}

/// Where yt-dlp writes the finished file.
///
/// The result is a literal path plus yt-dlp's extension placeholder, and nothing else. Our
/// own template grammar is expanded here, *before* the value is handed over, so none of
/// yt-dlp's `%(field)s` syntax is reachable from anything a site or a user supplied — the
/// `-o` argument stays an opaque literal.
///
/// A template that cannot be expanded — an unknown field, traversal from a hostile title,
/// nothing left after substitution — falls back to the plain file name rather than failing
/// the download. The template was already validated when it was saved, so reaching this is
/// a data problem with one particular page, not a configuration error worth stopping for.
fn output_template(
    directory: &std::path::Path,
    stem: &str,
    template: Option<&str>,
    values: &rd_files::TemplateValues,
) -> PathBuf {
    let plain = || with_ext_placeholder(&directory.join(stem));
    let Some(template) = template.map(str::trim).filter(|value| !value.is_empty()) else {
        return plain();
    };
    match rd_files::expand(directory, template, values, YTDLP_SUFFIX_RESERVE) {
        Ok(path) => with_ext_placeholder(&path),
        Err(error) => {
            tracing::warn!(%error, "output template could not be expanded; using the file name");
            plain()
        }
    }
}

/// Appends yt-dlp's extension placeholder to a path that has to stay literal.
///
/// Every `%` already in the path is doubled first. `rd_files::sanitize_file_name` replaces
/// the characters a file system objects to — `<>:"/\|?*` and the control characters — and
/// leaves `%` alone, because a per cent sign is a perfectly ordinary character in a file
/// name. It is not an ordinary character to yt-dlp: without this, a page titled
/// `50%(title)s off` reaches it as a real output template, so the file lands under a name
/// nobody chose or the job fails outright on an unknown field. `%%` is yt-dlp's escape for a
/// literal per cent and collapses back to one character on disk, which is also why doubling
/// does not overrun the length budget `rd_files::sanitize_file_name_within` reserved: the
/// argument grows, the file that gets written does not.
///
/// The escaping belongs here rather than in `rd-files`: the rule is yt-dlp's, not a general
/// file-name rule, and every other consumer of a sanitised name wants the `%` left alone.
fn with_ext_placeholder(path: &std::path::Path) -> PathBuf {
    let mut argument = match path.to_str() {
        Some(text) => std::ffi::OsString::from(text.replace('%', "%%")),
        // A path that is not valid UTF-8 cannot be rewritten without losing bytes, so it is
        // handed over as it stands rather than mangled. Every path this crate builds comes
        // out of the sanitiser, so this is the theoretical branch.
        None => path.as_os_str().to_owned(),
    };
    argument.push(".%(ext)s");
    PathBuf::from(argument)
}

/// The values an output template is expanded against.
///
/// Deliberately narrow: exactly the allowlisted fields, taken from what the selection
/// already carries. Nothing here reaches back into the extractor's raw metadata.
fn template_values(selection: &MediaSelection, stem: &str) -> rd_files::TemplateValues {
    let mut values = rd_files::TemplateValues::new();
    let title = if selection.title.trim().is_empty() {
        stem.to_owned()
    } else {
        selection.title.clone()
    };
    values.insert("title".to_owned(), title);
    values.insert("ext".to_owned(), selection.ext.clone());
    if let Some(resolved) = selection.resolved.as_deref() {
        if !resolved.label.is_empty() {
            values.insert("resolution".to_owned(), resolved.label.clone());
        }
        if !resolved.container.is_empty() {
            values.insert("ext".to_owned(), resolved.container.clone());
        }
    }
    values
}

/// The `-f` expression for one download.
///
/// A [`MediaStrictness::Preferred`] selection rides on the stored expression: it already
/// lists the pinned ids, the same choice expressed semantically, and a merge-free last
/// resort, which yt-dlp evaluates left to right. Only a `Required` selection re-probes,
/// because only there does silently accepting the next alternative amount to handing over
/// something the user explicitly refused.
async fn resolve_format(
    ytdlp: &std::path::Path,
    settings: &rd_core::MediaSettings,
    selection: &MediaSelection,
    criteria: Option<&MediaFormatCriteria>,
    capabilities: MediaCapabilities,
) -> Result<String, Failure> {
    let Some(criteria) =
        criteria.filter(|criteria| criteria.strictness == rd_core::MediaStrictness::Required)
    else {
        return Ok(degrade(&selection.format, selection.kind, capabilities));
    };
    let timeout = Duration::from_secs(u64::from(settings.media_check_timeout_seconds.max(5)));
    let inventory = crate::probe::probe_inventory(ytdlp, &selection.page_url, timeout).await?;
    let resolution = resolve(&inventory, criteria, capabilities).map_err(|error| {
        let failure = Failure::coded(FailureKind::Permanent, error.code(), error.to_string());
        match &error {
            rd_core::MediaSelectionError::NoMatch { unsatisfiable, .. } => failure.with_param(
                "criteria",
                unsatisfiable
                    .iter()
                    .map(|criterion| criterion.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            rd_core::MediaSelectionError::NoFormats
            | rd_core::MediaSelectionError::MergeRequired
            | rd_core::MediaSelectionError::NoAudio => failure,
        }
    })?;
    Ok(resolution.format_expression)
}

/// Drops the merge alternatives when the tools cannot merge.
///
/// Without ffmpeg a merged format leaves two unusable stream files behind, so ask for a
/// single pre-muxed one instead. Warnings stay on — they carry exactly the "ffmpeg is not
/// installed" diagnostics that used to be swallowed.
fn degrade(format: &str, kind: MediaKind, capabilities: MediaCapabilities) -> String {
    if kind == MediaKind::Video && !capabilities.can_merge {
        progressive_format(format)
    } else {
        format.to_owned()
    }
}

/// Downloads `DownloadKind::Media` files.
pub struct MediaRunner {
    database: Database,
    secrets: SecretStore,
    settings: SharedMediaSettings,
    slot_capacity: usize,
}

impl MediaRunner {
    #[must_use]
    pub fn new(database: Database, secrets: SecretStore, settings: SharedMediaSettings) -> Self {
        let slot_capacity = settings
            .try_read()
            .map(|guard| guard.media_max_parallel.clamp(1, 8) as usize)
            .unwrap_or(2);
        Self {
            database,
            secrets,
            settings,
            slot_capacity,
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

    fn slot_capacity(&self) -> usize {
        self.slot_capacity
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
        // Leased, not just located, and leased *before* the version is assessed: a managed
        // yt-dlp stays on disk for as long as this download runs even if another version is
        // activated meanwhile (RD-102-02). A yt-dlp this build does not support cannot be
        // relied on to produce the file that was selected, so media downloads stop and the
        // rest of the queue is untouched (RD-102-03). Both rules live in `prepare`.
        let ytdlp_tool = match prepare(
            "yt-dlp",
            lease_tool(
                settings.media_ytdlp_executable.as_deref(),
                settings.vendor_directory.as_deref(),
                "yt-dlp",
            ),
            rd_tools::Capability::MediaDownload,
        )
        .await
        {
            Ok(tool) => tool,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        // Bound for the whole run: dropping the `PreparedTool` releases the lease, so it may
        // not be reduced to its path here.
        let ytdlp = ytdlp_tool.path();
        let ffmpeg = FfmpegTools::resolve(&settings);
        // MP3 conversion cannot happen without the tools, so fail before downloading. A video
        // still succeeds: yt-dlp then falls back to a progressive format instead of merging.
        if selection.kind == MediaKind::Audio
            && let Some(blocking) = ffmpeg.blocking(rd_tools::Capability::AudioExtraction).await
        {
            return Ok(RunOutcome::Failed(rd_tools::compat::incompatible_failure(
                &blocking,
                rd_tools::Capability::AudioExtraction,
            )));
        }
        if selection.kind == MediaKind::Audio && !ffmpeg.is_complete() {
            let missing = if ffmpeg.ffmpeg.is_none() {
                "ffmpeg"
            } else {
                "ffprobe"
            };
            return Ok(RunOutcome::Failed(
                Failure::coded(
                    FailureKind::Unsupported,
                    "media.tool_missing",
                    format!("{missing} is required for MP3 conversion but is not installed"),
                )
                .with_param("tool", missing),
            ));
        }
        if selection.kind == MediaKind::Video && !ffmpeg.is_complete() {
            tracing::warn!(
                ffmpeg = ffmpeg.ffmpeg.is_some(),
                ffprobe = ffmpeg.ffprobe.is_some(),
                "ffmpeg and ffprobe are both required to merge video and audio; \
                 yt-dlp will fall back to a single pre-muxed stream"
            );
        }
        let directory = PathBuf::from(&package.destination);
        tokio::fs::create_dir_all(&directory).await?;
        // yt-dlp writes intermediate files next to the target (`<stem>.f137.mp4.part`), so the
        // stem has to leave room for the longest suffix it appends, not just for `.%(ext)s`.
        let stem = rd_files::sanitize_file_name_within(
            &directory,
            std::path::Path::new(&file.file_name)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("media"),
            YTDLP_SUFFIX_RESERVE,
        );
        let capabilities = ffmpeg.media_capabilities().await;
        let criteria = selection.effective_criteria();
        // A per-job template beats the configured default; neither is required.
        let output_pattern = criteria
            .as_ref()
            .and_then(|criteria| criteria.output_template.clone())
            .or_else(|| settings.media_output_template.clone());
        let template = output_template(
            &directory,
            &stem,
            output_pattern.as_deref(),
            &template_values(&selection, &stem),
        );
        // Bound here, before the spawn, and dropped when `run` returns by any path — the
        // success return, an early `Failed`, and the cancellation branch that kills the
        // child all unwind through this binding, so the file cannot outlive the download.
        let cookies = match self.cookie_file(file, &selection).await {
            Ok(cookies) => cookies,
            Err(error) => {
                return Ok(RunOutcome::Failed(Failure::coded(
                    FailureKind::Permanent,
                    error.code(),
                    error.message(),
                )));
            }
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
        // The policy is reduced to what this container and these tools actually allow
        // *before* the arguments are built, so the flags and what the UI previewed agree.
        let embed = criteria
            .as_ref()
            .map_or_else(rd_core::MediaEmbedPolicy::default, |criteria| {
                rd_core::effective_policy(
                    &criteria.embed,
                    &selection.ext,
                    &selection.page_url,
                    capabilities.can_transcode_audio,
                )
            });
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
        let mut process = ToolProcess::spawn(&mut command, "yt-dlp", Stdout::Read)?;
        let stdout = process.take_stdout().context("yt-dlp stdout")?;
        let mut lines = BufReader::new(stdout).lines();
        let mut final_path: Option<String> = None;
        let mut throttle = ProgressThrottle::default();
        // A merged video is fetched as two streams that each count 0-100 %. Bytes of the
        // finished streams are carried over so the reported progress only ever grows.
        let mut finished_bytes: u64 = 0;
        let mut stream_total: Option<u64> = None;
        let mut stream_committed: u64 = 0;
        loop {
            let line = tokio::select! {
                () = cancellation.cancelled() => {
                    process.kill().await;
                    return Ok(RunOutcome::Stopped);
                }
                line = lines.next_line() => line?,
            };
            let Some(line) = line else { break };
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
        let Some(path) = final_path.map(PathBuf::from) else {
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: Some(120),
                },
                "media.output_missing",
                "yt-dlp finished without reporting the output file",
            )));
        };
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{final_path_line, output_template, progressive_format};

    #[test]
    fn keeps_only_the_alternative_that_needs_no_merge() {
        assert_eq!(
            progressive_format("bv*[height<=1080]+ba/b[height<=1080]"),
            "b[height<=1080]"
        );
        assert_eq!(progressive_format("bv*+ba/b"), "b");
        // Nothing pre-muxed on offer: fall back to yt-dlp's own "best single file".
        assert_eq!(progressive_format("bv*+ba"), "b");
        assert_eq!(progressive_format("b"), "b");
    }

    #[test]
    fn without_a_template_the_output_is_the_plain_file_name() {
        let values = rd_files::TemplateValues::new();
        assert_eq!(
            output_template(Path::new("/downloads/pkg"), "clip", None, &values),
            Path::new("/downloads/pkg/clip.%(ext)s")
        );
        // An empty pattern is the same as none, not an empty path.
        assert_eq!(
            output_template(Path::new("/downloads/pkg"), "clip", Some("   "), &values),
            Path::new("/downloads/pkg/clip.%(ext)s")
        );
    }

    #[test]
    fn a_template_produces_a_literal_path_with_only_ytdlps_extension_placeholder() {
        let mut values = rd_files::TemplateValues::new();
        values.insert("title".to_owned(), "Trailer".to_owned());
        values.insert("uploader".to_owned(), "Studio".to_owned());
        let path = output_template(
            Path::new("/downloads/pkg"),
            "clip",
            Some("{uploader}/{title}"),
            &values,
        );
        assert_eq!(path, Path::new("/downloads/pkg/Studio/Trailer.%(ext)s"));
        // Nothing but the extension placeholder survives into the argument.
        let rendered = path.to_string_lossy();
        assert_eq!(rendered.matches('%').count(), 1);
    }

    #[test]
    fn a_per_cent_in_a_name_is_escaped_so_yt_dlp_cannot_read_it_as_a_field() {
        // `sanitize_file_name` leaves `%` alone, so without the doubling this title reaches
        // yt-dlp as a real output template and the file lands under a name nobody chose.
        let values = rd_files::TemplateValues::new();
        let path = output_template(
            Path::new("/downloads/pkg"),
            "50%(title)s off",
            None,
            &values,
        );
        assert_eq!(path, Path::new("/downloads/pkg/50%%(title)s off.%(ext)s"));

        let mut values = rd_files::TemplateValues::new();
        values.insert("title".to_owned(), "100% Wolf".to_owned());
        let path = output_template(
            Path::new("/downloads/pkg"),
            "clip",
            Some("{title}"),
            &values,
        );
        assert_eq!(path, Path::new("/downloads/pkg/100%% Wolf.%(ext)s"));
        // The only unescaped placeholder left is the extension yt-dlp fills in.
        assert_eq!(path.to_string_lossy().matches("%(").count(), 1);
    }

    #[test]
    fn only_the_marked_line_is_taken_as_the_output_path() {
        assert_eq!(
            final_path_line("rdownloader-final-path:/downloads/pkg/clip.mp4"),
            Some("/downloads/pkg/clip.mp4")
        );
        // The lines that used to overwrite the path: anything unprefixed on stdout.
        assert_eq!(final_path_line("/downloads/pkg/wrong.mp4"), None);
        assert_eq!(final_path_line("WARNING: generic extractor"), None);
        assert_eq!(
            final_path_line("[download] Destination: clip.f137.mp4"),
            None
        );
        assert_eq!(final_path_line(""), None);
        assert_eq!(final_path_line("   "), None);
        // A marker with nothing behind it is not an answer either.
        assert_eq!(final_path_line("rdownloader-final-path:"), None);
        assert_eq!(final_path_line("rdownloader-final-path:   "), None);
    }

    #[test]
    fn a_template_that_cannot_be_expanded_falls_back_instead_of_failing_the_download() {
        // The template was validated when it was saved; a title of `..` is a problem with
        // one page, not with the configuration, so the file still lands somewhere sane.
        let mut values = rd_files::TemplateValues::new();
        values.insert("title".to_owned(), "..".to_owned());
        assert_eq!(
            output_template(
                Path::new("/downloads/pkg"),
                "clip",
                Some("{title}"),
                &values
            ),
            Path::new("/downloads/pkg/clip.%(ext)s")
        );
    }
}

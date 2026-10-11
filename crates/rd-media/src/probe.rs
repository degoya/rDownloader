//! Metadata probe: `yt-dlp -J --flat-playlist` for one page, or for a playlist's listing.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use async_trait::async_trait;
use rd_core::{
    Failure, FailureKind, MediaCandidate, MediaCandidateState, MediaFormatInventory, MediaInfo,
};
use rd_scheduler::{ToolNetwork, ToolNetworkSource};
use rd_tools::process::prepare;
use serde::Deserialize;
use tokio::sync::Semaphore;
use url::Url;

use crate::{
    SharedMediaSettings,
    format_inventory::normalize,
    select::MediaCapabilities,
    tools::{FfmpegTools, lease_tool},
    tracks::{RawSubtitleMap, audio_tracks, subtitle_tracks},
    variants::synthesize_from_inventory,
};

/// Maximum playlist entries expanded into separate candidates.
pub(crate) const MAX_PLAYLIST_ENTRIES: usize = 200;

/// Source of media metadata (mockable in tests).
#[async_trait]
pub trait MediaProbe: Send + Sync {
    /// One entry for a single page, several for a playlist/channel (capped).
    async fn probe(&self, url: &Url) -> Result<Vec<MediaCandidate>, Failure>;
}

/// Probe backed by the configured `yt-dlp` executable.
pub struct YtDlpProbe {
    settings: SharedMediaSettings,
    slots: Arc<Semaphore>,
    /// The proxy and CA the probe goes through (RD-1240-22); none without it, as in tests.
    network: Option<ToolNetworkSource>,
}

impl YtDlpProbe {
    #[must_use]
    pub fn new(settings: SharedMediaSettings) -> Self {
        Self {
            settings,
            slots: Arc::new(Semaphore::new(2)),
            network: None,
        }
    }

    /// Sends the probe through the global proxy profile and the custom CA (RD-1240-22): a
    /// link in the LinkGrabber belongs to no download yet, so to no job's or account's profile.
    #[must_use]
    pub fn with_tool_network(mut self, network: ToolNetworkSource) -> Self {
        self.network = Some(network);
        self
    }
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "_type")]
    kind: Option<String>,
    title: Option<String>,
    duration: Option<f64>,
    uploader: Option<String>,
    thumbnail: Option<String>,
    webpage_url: Option<String>,
    upload_date: Option<String>,
    extractor: Option<String>,
    id: Option<String>,
    #[serde(default)]
    formats: Vec<crate::RawFormat>,
    /// Authored subtitle tracks, keyed by language.
    #[serde(default)]
    subtitles: RawSubtitleMap,
    /// Speech-recognition tracks, keyed by language.
    #[serde(default)]
    automatic_captions: RawSubtitleMap,
    #[serde(default)]
    entries: Vec<PlaylistEntry>,
}

#[derive(Deserialize)]
struct PlaylistEntry {
    title: Option<String>,
    url: Option<String>,
    webpage_url: Option<String>,
    id: Option<String>,
    duration: Option<f64>,
    uploader: Option<String>,
    #[serde(default)]
    thumbnails: Vec<Thumbnail>,
}

#[derive(Deserialize)]
struct Thumbnail {
    url: Option<String>,
}

#[async_trait]
impl MediaProbe for YtDlpProbe {
    async fn probe(&self, url: &Url) -> Result<Vec<MediaCandidate>, Failure> {
        let _permit = self.slots.clone().acquire_owned().await.map_err(|_| {
            Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "media.probe_busy",
                "Media probe is shutting down",
            )
        })?;
        // Before anything reaches the network: a proxy that cannot be used is a failure, never
        // a direct connection.
        let network = match &self.network {
            Some(source) => source.for_request(url).await?,
            None => ToolNetwork::direct(),
        };
        let settings = self.settings.read().await.clone();
        // The lease keeps the managed version alive for the length of the probe run, and it is
        // taken before the version is assessed (RD-102-02). A yt-dlp below the floor this build
        // is tested against cannot probe reliably, so it is refused here rather than allowed to
        // produce a variant list nothing can download; only media probing stops and the rest of
        // the queue is untouched (RD-102-03). Both rules live in `prepare`, which is what the
        // three runners already call — this was the last copy of them.
        let ytdlp_tool = prepare(
            "yt-dlp",
            lease_tool(
                settings.media_ytdlp_executable.as_deref(),
                settings.vendor_directory.as_deref(),
                "yt-dlp",
            ),
            rd_tools::Capability::MediaDownload,
        )
        .await?;
        // Bound for the whole probe: dropping the `PreparedTool` releases the lease, so it may
        // not be reduced to its path here.
        let ytdlp = ytdlp_tool.path();
        let timeout = Duration::from_secs(u64::from(settings.media_check_timeout_seconds.max(5)));
        // The variants offered must already respect what the tools can do, so that nobody
        // can pick a merge on an installation that cannot merge — which now includes an
        // ffmpeg whose version is too old or listed as broken.
        let ffmpeg = FfmpegTools::resolve(&settings);
        let capabilities = ffmpeg.media_capabilities().await;
        let preferred = settings.media_default_variant.clone();
        // One flat run (RD-1240-37). A full `-J` came first before, and on a playlist address
        // it resolved every video only to learn that the page is a playlist: 196 s and exit 1
        // for a 240-entry YouTube list, where one private or removed video failed the whole
        // run. `--flat-playlist` lists a playlist's entries and still probes a single page in
        // full, formats included.
        let metadata = run_json(ytdlp, url, true, timeout, &network).await?;
        candidates(metadata, url, &preferred, capabilities)
    }
}

/// One candidate for a single page, one per entry for a playlist (capped).
fn candidates(
    metadata: Metadata,
    url: &Url,
    preferred: &str,
    capabilities: MediaCapabilities,
) -> Result<Vec<MediaCandidate>, Failure> {
    if metadata.kind.as_deref() != Some("playlist") {
        return Ok(vec![single(metadata, url, preferred, capabilities)]);
    }
    let entries: Vec<MediaCandidate> = metadata
        .entries
        .into_iter()
        .filter(|entry| !unavailable(entry))
        .take(MAX_PLAYLIST_ENTRIES)
        .filter_map(|entry| playlist_entry(entry, preferred, capabilities))
        .collect();
    if entries.is_empty() {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "media.playlist_empty",
            "Playlist contains no downloadable entries",
        ));
    }
    Ok(entries)
}

/// YouTube's stand-in for a private or deleted video in a playlist: the flat listing names it
/// by this title and knows nothing else about it, and it can never be downloaded.
fn unavailable(entry: &PlaylistEntry) -> bool {
    entry.duration.is_none()
        && matches!(
            entry.title.as_deref(),
            Some("[Private video]" | "[Deleted video]")
        )
}

/// Re-probes `url` for its format inventory only.
///
/// Used by the runner when a selection is [`rd_core::MediaStrictness::Required`]: the
/// stored expression's alternatives are a best-effort degradation, which is exactly what a
/// required selection must not accept.
pub(crate) async fn probe_inventory(
    ytdlp: &std::path::Path,
    url: &Url,
    timeout: Duration,
    network: &ToolNetwork,
) -> Result<MediaFormatInventory, Failure> {
    let metadata = run_json(ytdlp, url, false, timeout, network).await?;
    Ok(normalize(&metadata.formats))
}

async fn run_json(
    ytdlp: &std::path::Path,
    url: &Url,
    flat: bool,
    timeout: Duration,
    network: &ToolNetwork,
) -> Result<Metadata, Failure> {
    let mut command = crate::args::ytdlp_command(ytdlp, network);
    // `--no-playlist` keeps a video address that also names a list (`watch?v=…&list=…`) the
    // one video.
    command.args(["-J", "--no-warnings", "--no-color", "--no-playlist"]);
    if flat {
        // Entries are listed, not resolved, and the listing stops where the expansion would.
        command
            .args(["--flat-playlist", "--playlist-end"])
            .arg(MAX_PLAYLIST_ENTRIES.to_string());
    }
    command.arg("--").arg(url.as_str());
    // The stdio wiring, the timeout and the Windows console-window flag are the same for
    // every short tool invocation and live in rd-tools; stdout and stderr are both captured,
    // because `Command::output` pipes them whatever the caller asks for.
    let output = rd_tools::process::run_to_output(&mut command, timeout)
        .await
        .map_err(|_| {
            Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: Some(60),
                },
                "media.probe_timeout",
                "Media probe timed out",
            )
        })?
        .context("spawn yt-dlp")
        // With its cause: "spawn yt-dlp" alone does not say the file is missing or not
        // executable (re-audit 1.9.1, RA-TR-04).
        .map_err(|error| tool_failure(format!("{error:#}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(network
            .unsupported_proxy("yt-dlp", &stderr)
            .unwrap_or_else(|| map_tool_error(&stderr, output.status.code())));
    }
    serde_json::from_slice::<Metadata>(&output.stdout)
        .map_err(|error| tool_failure(format!("invalid yt-dlp JSON: {error}")))
}

fn single(
    metadata: Metadata,
    url: &Url,
    preferred: &str,
    capabilities: MediaCapabilities,
) -> MediaCandidate {
    let inventory = normalize(&metadata.formats);
    let (variants, selected) = synthesize_from_inventory(&inventory, preferred, capabilities);
    let criteria = variants
        .iter()
        .find(|variant| variant.id == selected)
        .and_then(|variant| variant.criteria.clone())
        .unwrap_or_default();
    let audio = audio_tracks(&inventory);
    let subtitles = subtitle_tracks(&metadata.subtitles, &metadata.automatic_captions);
    let info = MediaInfo {
        title: metadata
            .title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "media".to_owned()),
        duration_seconds: metadata.duration.map(|value| value.round() as u32),
        uploader: metadata.uploader,
        thumbnail: metadata.thumbnail,
        page_url: metadata
            .webpage_url
            .as_deref()
            .and_then(web_page)
            .unwrap_or_else(|| url.clone()),
        variants,
        selected,
        upload_date: metadata.upload_date,
        extractor: metadata.extractor,
        video_id: metadata.id,
    };
    MediaCandidate {
        info,
        state: MediaCandidateState {
            audio_tracks: audio,
            subtitles,
            ..MediaCandidateState::probed(inventory, criteria)
        },
    }
}

fn playlist_entry(
    entry: PlaylistEntry,
    preferred: &str,
    capabilities: MediaCapabilities,
) -> Option<MediaCandidate> {
    let page = entry
        .webpage_url
        .or(entry.url)
        .and_then(|value| web_page(&value))
        .or_else(|| {
            entry
                .id
                .as_deref()
                .and_then(|id| Url::parse(&format!("https://www.youtube.com/watch?v={id}")).ok())
        })?;
    // A flat playlist listing carries no formats, and nothing probes the entry again before
    // it is queued. The empty inventory therefore resolves no preset, and the fallback's
    // `best` (plus MP3 where ffmpeg can extract) is what gives the entry a selection at all
    // (RD-120-50); yt-dlp picks the concrete format at download time.
    let (variants, selected) =
        synthesize_from_inventory(&MediaFormatInventory::default(), preferred, capabilities);
    let info = MediaInfo {
        title: entry
            .title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "media".to_owned()),
        duration_seconds: entry.duration.map(|value| value.round() as u32),
        uploader: entry.uploader,
        thumbnail: entry.thumbnails.into_iter().find_map(|thumb| thumb.url),
        page_url: page,
        variants,
        selected,
        upload_date: None,
        extractor: None,
        video_id: entry.id,
    };
    Some(MediaCandidate {
        info,
        state: MediaCandidateState::default(),
    })
}

/// Maps yt-dlp's stderr and exit code to a failure class the queue can retry or block on.
///
/// Read from the `ERROR:` line alone and by whole words (audit 1.9.1, TR-02): matched as a
/// substring of all of stderr, "age" found "Unable to download web*page*" — a network failure
/// that then counted as a login wall and was never retried — and a warning's "404" or
/// "removed" made a passing problem permanent.
pub(crate) fn map_tool_error(stderr: &str, exit_code: Option<i32>) -> Failure {
    let text = stderr.trim();
    if crate::merge::merge_skipped(text) {
        return crate::merge::merge_failure();
    }
    let error_line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("ERROR:"))
        .unwrap_or_else(|| text.lines().last().unwrap_or_default());
    // Before the rest: a proxy refusing the profile's password was a network failure retried
    // every two minutes, never the proxy's (RD-1240-29).
    if let Some(failure) = rd_scheduler::proxy_auth_failed(error_line) {
        return failure;
    }
    let lower = error_line.to_ascii_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| contains_words(&lower, phrase));
    // Redacted before it becomes a message and a param: this tail is shown in the UI and
    // stored on the download row, and yt-dlp happily echoes the signed URL it just tried.
    let tail: String = rd_core::redact_text(error_line).chars().take(300).collect();
    // Never an empty detail: "yt-dlp failed: " with nothing after it names no cause at all
    // (RD-1240-37). A run that printed nothing still has its exit code to say.
    let tail = if tail.is_empty() {
        match exit_code {
            Some(code) => format!("exit code {code}, no error output"),
            None => "ended by a signal, no error output".to_owned(),
        }
    } else {
        tail
    };
    let category = if says(&[
        "unsupported url",
        "video unavailable",
        "private video",
        "not available",
        "removed",
        "404",
    ]) {
        FailureKind::Permanent
    } else if says(&["sign in", "login", "log in", "age"]) {
        FailureKind::AuthRequired
    } else if says(&["429", "too many requests"]) {
        FailureKind::RateLimited {
            retry_after_seconds: Some(600),
        }
    } else {
        FailureKind::Transient {
            retry_after_seconds: Some(120),
        }
    };
    Failure::coded(
        category,
        "media.ytdlp_failed",
        format!("yt-dlp failed: {tail}"),
    )
    .with_param("detail", tail)
}

/// Whether `phrase` occurs in `text` with no letter or digit directly before or after it.
fn contains_words(text: &str, phrase: &str) -> bool {
    let is_word = |character: Option<char>| character.is_some_and(char::is_alphanumeric);
    text.match_indices(phrase).any(|(start, _)| {
        !is_word(text[..start].chars().next_back())
            && !is_word(text[start + phrase.len()..].chars().next())
    })
}

fn tool_failure(detail: String) -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: Some(120),
        },
        "media.tool_error",
        format!("Media tool error: {detail}"),
    )
    .with_param("detail", detail)
}

/// A reported page address the interface may use as a link: http(s) only (audit K9).
fn web_page(value: &str) -> Option<Url> {
    let page = Url::parse(value).ok()?;
    matches!(page.scheme(), "http" | "https").then_some(page)
}

#[cfg(test)]
#[path = "probe_page_tests.rs"]
mod page_tests;

#[cfg(test)]
#[path = "probe_error_tests.rs"]
mod error_tests;

#[cfg(test)]
#[path = "probe_playlist_tests.rs"]
mod playlist_tests;

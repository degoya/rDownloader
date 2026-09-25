//! Metadata probe: `yt-dlp -J` for one page (or a playlist, flattened).

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use async_trait::async_trait;
use rd_core::{
    Failure, FailureKind, MediaCandidate, MediaCandidateState, MediaFormatInventory, MediaInfo,
};
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
pub const MAX_PLAYLIST_ENTRIES: usize = 200;

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
}

impl YtDlpProbe {
    #[must_use]
    pub fn new(settings: SharedMediaSettings) -> Self {
        Self {
            settings,
            slots: Arc::new(Semaphore::new(2)),
        }
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
        let metadata = run_json(ytdlp, url, false, timeout).await?;
        if metadata.kind.as_deref() != Some("playlist") {
            return Ok(vec![single(metadata, url, &preferred, capabilities)]);
        }
        let flat = run_json(ytdlp, url, true, timeout).await?;
        let entries: Vec<MediaCandidate> = flat
            .entries
            .into_iter()
            .take(MAX_PLAYLIST_ENTRIES)
            .filter_map(|entry| playlist_entry(entry, &preferred, capabilities))
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
) -> Result<MediaFormatInventory, Failure> {
    let metadata = run_json(ytdlp, url, false, timeout).await?;
    Ok(normalize(&metadata.formats))
}

async fn run_json(
    ytdlp: &std::path::Path,
    url: &Url,
    flat: bool,
    timeout: Duration,
) -> Result<Metadata, Failure> {
    let mut command = tokio::process::Command::new(ytdlp);
    command
        .args(["-J", "--no-warnings", "--no-color"])
        .arg(if flat {
            "--flat-playlist"
        } else {
            "--no-playlist"
        })
        .arg("--")
        .arg(url.as_str());
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
        .map_err(|error| tool_failure(error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(map_tool_error(&stderr));
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
            .and_then(|value| Url::parse(value).ok())
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
        .and_then(|value| Url::parse(&value).ok())
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

/// Maps yt-dlp's stderr to a failure class the queue can retry or block on.
pub(crate) fn map_tool_error(stderr: &str) -> Failure {
    let text = stderr.trim();
    let lower = text.to_ascii_lowercase();
    // Redacted before it becomes a message and a param: this tail is shown in the UI and
    // stored on the download row, and yt-dlp happily echoes the signed URL it just tried.
    let tail: String = rd_core::redact_text(text.lines().last().unwrap_or_default())
        .chars()
        .take(300)
        .collect();
    let category = if lower.contains("unsupported url")
        || lower.contains("video unavailable")
        || lower.contains("private video")
        || lower.contains("not available")
        || lower.contains("removed")
        || lower.contains("404")
    {
        FailureKind::Permanent
    } else if lower.contains("sign in") || lower.contains("login") || lower.contains("age") {
        FailureKind::AuthRequired
    } else if lower.contains("429") || lower.contains("too many requests") {
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

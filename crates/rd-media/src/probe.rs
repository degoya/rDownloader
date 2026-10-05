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
        // With its cause: "spawn yt-dlp" alone does not say the file is missing or not
        // executable (re-audit 1.9.1, RA-TR-04).
        .map_err(|error| tool_failure(format!("{error:#}")))?;
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

/// Maps yt-dlp's stderr to a failure class the queue can retry or block on.
///
/// Read from the `ERROR:` line alone and by whole words (audit 1.9.1, TR-02): matched as a
/// substring of all of stderr, "age" found "Unable to download web*page*" — a network failure
/// that then counted as a login wall and was never retried — and a warning's "404" or
/// "removed" made a passing problem permanent.
pub(crate) fn map_tool_error(stderr: &str) -> Failure {
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
    let lower = error_line.to_ascii_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| contains_words(&lower, phrase));
    // Redacted before it becomes a message and a param: this tail is shown in the UI and
    // stored on the download row, and yt-dlp happily echoes the signed URL it just tried.
    let tail: String = rd_core::redact_text(error_line).chars().take(300).collect();
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
mod error_tests {
    use rd_core::FailureKind;

    use super::{contains_words, map_tool_error};

    fn category(stderr: &str) -> FailureKind {
        map_tool_error(stderr).category
    }

    /// RA-TR-04: a yt-dlp that cannot be started says why, not only "spawn yt-dlp".
    #[tokio::test]
    async fn a_tool_that_cannot_start_reports_the_cause() {
        let missing = tempfile::tempdir().expect("temp");
        let failure = super::run_json(
            &missing.path().join("no-such-yt-dlp"),
            &"https://example.invalid/watch".parse().expect("url"),
            false,
            std::time::Duration::from_secs(10),
        )
        .await;
        let Err(failure) = failure else {
            panic!("nothing to start")
        };
        assert_eq!(failure.code.as_deref(), Some("media.tool_error"));
        let cause = failure
            .message
            .split_once("spawn yt-dlp: ")
            .map(|(_, cause)| cause.trim())
            .unwrap_or_default();
        assert!(!cause.is_empty(), "the cause was lost: {}", failure.message);
    }

    /// TR-02: yt-dlp's own wording, as it prints it.
    #[test]
    fn a_network_failure_is_retried_rather_than_taken_for_a_login_wall() {
        assert_eq!(
            category(
                "ERROR: [youtube] dQw4w9WgXcQ: Unable to download webpage: <urlopen error \
                 [Errno -3] Temporary failure in name resolution> (caused by \
                 URLError(gaierror(-3, 'Temporary failure in name resolution')))"
            ),
            FailureKind::Transient {
                retry_after_seconds: Some(120)
            }
        );
        assert_eq!(
            category(
                "ERROR: [generic] Unable to download webpage: HTTP Error 503: Service Unavailable"
            ),
            FailureKind::Transient {
                retry_after_seconds: Some(120)
            }
        );
    }

    #[test]
    fn a_warning_does_not_decide_the_class() {
        let stderr = "WARNING: [youtube] Video 3 of the playlist was removed (HTTP 404)\n\
                      WARNING: Falling back to generic n function search\n\
                      ERROR: [youtube] dQw4w9WgXcQ: Unable to extract initial player response; \
                      please report this issue on https://github.com/yt-dlp/yt-dlp/issues";
        assert_eq!(
            category(stderr),
            FailureKind::Transient {
                retry_after_seconds: Some(120)
            }
        );
    }

    #[test]
    fn age_and_sign_in_walls_need_an_account() {
        for stderr in [
            "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm your age. This video may be \
             inappropriate for some users.",
            "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you\u{2019}re not a bot. Use \
             --cookies-from-browser or --cookies for the authentication.",
            "ERROR: [vimeo] 123456: This video is age-restricted",
        ] {
            assert_eq!(category(stderr), FailureKind::AuthRequired, "{stderr}");
        }
    }

    #[test]
    fn gone_videos_are_permanent() {
        for stderr in [
            "ERROR: [youtube] dQw4w9WgXcQ: Video unavailable. This video has been removed by \
             the uploader",
            "ERROR: [youtube] dQw4w9WgXcQ: Private video. Sign in if you've been granted \
             access to this video",
            "ERROR: Unsupported URL: https://example.com/page",
            "ERROR: [generic] Unable to download webpage: HTTP Error 404: Not Found",
        ] {
            assert_eq!(category(stderr), FailureKind::Permanent, "{stderr}");
        }
    }

    #[test]
    fn too_many_requests_waits_longer() {
        assert_eq!(
            category("ERROR: unable to download video data: HTTP Error 429: Too Many Requests"),
            FailureKind::RateLimited {
                retry_after_seconds: Some(600)
            }
        );
    }

    #[test]
    fn words_match_whole_and_only_whole() {
        assert!(contains_words("confirm your age.", "age"));
        assert!(contains_words("age-restricted", "age"));
        for text in ["webpage", "message", "image", "storage", "usage limit"] {
            assert!(!contains_words(text, "age"), "{text}");
        }
        assert!(!contains_words("error 4040", "404"));
        assert!(contains_words("http error 404: not found", "404"));
    }

    #[test]
    fn the_detail_is_the_error_line() {
        let failure = map_tool_error("WARNING: something\nERROR: [youtube] x: Video unavailable\n");
        assert_eq!(
            failure.params.get("detail").map(String::as_str),
            Some("ERROR: [youtube] x: Video unavailable")
        );
    }
}

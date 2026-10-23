//! Metadata probe: `yt-dlp -J --flat-playlist` for one page, or for a playlist's listing.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use async_trait::async_trait;
use rd_core::{
    AuthProfileSelection, Failure, FailureKind, MediaCandidate, MediaCandidateState,
    MediaFormatInventory, MediaInfo,
};
use rd_db::Database;
use rd_scheduler::{ToolNetwork, ToolNetworkSource};
use rd_secrets::SecretStore;
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

#[path = "probe_error.rs"]
mod error;

#[cfg(test)]
use error::contains_words;
pub(crate) use error::map_tool_error;

/// Maximum playlist entries expanded into separate candidates.
pub(crate) const MAX_PLAYLIST_ENTRIES: usize = 200;

/// Source of media metadata (mockable in tests).
#[async_trait]
pub trait MediaProbe: Send + Sync {
    /// One entry for a single page, several for a playlist/channel (capped).
    ///
    /// `auth` is the link's authentication profile choice, resolved by the same rules as the
    /// download's (RD-1320-02): a matching cookie profile reaches yt-dlp as `--cookies`.
    async fn probe(
        &self,
        url: &Url,
        auth: AuthProfileSelection,
    ) -> Result<Vec<MediaCandidate>, Failure>;
}

/// Probe backed by the configured `yt-dlp` executable.
pub struct YtDlpProbe {
    settings: SharedMediaSettings,
    slots: Arc<Semaphore>,
    /// The proxy and CA the probe goes through (RD-1240-22); none without it, as in tests.
    network: Option<ToolNetworkSource>,
    /// Where the cookie profiles come from (RD-1320-02); none without it, as in tests, and
    /// then every probe runs without cookies.
    profiles: Option<(Database, SecretStore)>,
}

impl YtDlpProbe {
    #[must_use]
    pub fn new(settings: SharedMediaSettings) -> Self {
        Self {
            settings,
            slots: Arc::new(Semaphore::new(2)),
            network: None,
            profiles: None,
        }
    }

    /// Lets the probe use the authentication profiles the download uses (RD-1320-02).
    #[must_use]
    pub fn with_auth_profiles(mut self, database: Database, secrets: SecretStore) -> Self {
        self.profiles = Some((database, secrets));
        self
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
    /// The post's text; names the video when the title is only a placeholder.
    description: Option<String>,
    duration: Option<f64>,
    uploader: Option<String>,
    /// For a playlist, the channel it belongs to; the package's name when the list has no title.
    channel: Option<String>,
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
    async fn probe(
        &self,
        url: &Url,
        auth: AuthProfileSelection,
    ) -> Result<Vec<MediaCandidate>, Failure> {
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
        // The link's cookie profile, by the download's rules (RD-1320-02): a check without
        // the session tells a page behind a login apart from a broken one no better than a
        // guess. Bound until the run returns; the file deletes itself when the binding drops.
        let cookie_file = match &self.profiles {
            Some((database, secrets)) => {
                crate::cookies::for_selection(database, secrets, auth, url)
                    .await
                    .map_err(crate::CookieError::failure)?
            }
            None => None,
        };
        // One flat run (RD-1240-37). A full `-J` came first before, and on a playlist address
        // it resolved every video only to learn that the page is a playlist: 196 s and exit 1
        // for a 240-entry YouTube list, where one private or removed video failed the whole
        // run. `--flat-playlist` lists a playlist's entries and still probes a single page in
        // full, formats included.
        let cookies = cookie_file.as_ref().map(crate::CookieFile::path);
        let metadata = run_json(ytdlp, url, true, timeout, &network, cookies).await?;
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
    let playlist = playlist_title(&metadata);
    let entries: Vec<MediaCandidate> = metadata
        .entries
        .into_iter()
        .filter(|entry| !unavailable(entry))
        .take(MAX_PLAYLIST_ENTRIES)
        .filter_map(|entry| playlist_entry(entry, playlist.clone(), preferred, capabilities))
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

/// What the LinkGrabber names a playlist's package (RD-1250-03): its title, else its channel.
fn playlist_title(metadata: &Metadata) -> Option<String> {
    [&metadata.title, &metadata.channel, &metadata.uploader]
        .into_iter()
        .flatten()
        .map(|name| name.trim())
        .find(|name| !name.is_empty())
        .map(str::to_owned)
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
    cookies: Option<&std::path::Path>,
) -> Result<MediaFormatInventory, Failure> {
    let metadata = run_json(ytdlp, url, false, timeout, network, cookies).await?;
    Ok(normalize(&metadata.formats))
}

async fn run_json(
    ytdlp: &std::path::Path,
    url: &Url,
    flat: bool,
    timeout: Duration,
    network: &ToolNetwork,
    cookies: Option<&std::path::Path>,
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
    // The path, never the cookies: the file is the credential (`cookies.rs`).
    if let Some(cookies) = cookies {
        command.arg("--cookies").arg(cookies);
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
        title: media_title(&metadata),
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
        playlist_title: None,
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

/// A video's title, or for a placeholder title the first line of its text, else its uploader.
/// Facebook answers a signed-in request (cookies) with the title "Video" for every post (owner
/// report, 2026-10-22); signed out it names the post.
fn media_title(metadata: &Metadata) -> String {
    fn named(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    let title = named(metadata.title.as_deref());
    if let Some(title) = title.filter(|title| !title.eq_ignore_ascii_case("video")) {
        return title.to_owned();
    }
    let first_line = metadata
        .description
        .as_deref()
        .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()));
    first_line
        .or_else(|| named(metadata.uploader.as_deref()))
        .or(title)
        .unwrap_or("media")
        .to_owned()
}

fn playlist_entry(
    entry: PlaylistEntry,
    playlist_title: Option<String>,
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
        playlist_title,
    };
    Some(MediaCandidate {
        info,
        state: MediaCandidateState::default(),
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

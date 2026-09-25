//! Media provider: YouTube, dumpert.nl and other extractor-supported pages fetched through
//! external `yt-dlp` (+ `ffmpeg` for MP3/merging). Runs as an [`ExternalRunner`] of the
//! shared queue; metadata probes feed the LinkGrabber with selectable variants.

mod args;
mod cookies;
mod format_inventory;
mod manifest;
mod probe;
mod progress;
mod runner;
mod select;
mod tools;
mod tracks;
mod variants;

use std::sync::Arc;

use anyhow::Result;
use rd_core::MediaSettings;
use rd_db::Database;
use rd_scheduler::ExternalRunner;
use tokio::sync::RwLock;

pub use args::{DownloadPlan, output_mode};
pub use cookies::{CookieError, CookieFile, materialize as materialize_cookies, rows_for_url};
pub use format_inventory::{
    RawFormat, audio_codec_family, dynamic_range, normalize, video_codec_family,
};
pub use manifest::{
    DrmReason, MAX_MANIFEST_BYTES, ManifestClass, ManifestKind, ManifestReport, ManifestVariant,
    MediaRole, detect as detect_manifest, parse_dash, parse_hls, same_origin,
};
pub use probe::{MediaProbe, YtDlpProbe};
pub use progress::{DownloadProgressLine, parse_progress_line};
pub use runner::MediaRunner;
pub use select::{MediaCapabilities, extract_audio, remux_container, resolve};
pub use tools::{FfmpegTools, ToolStatus, locate_tool, tool_status};
pub use tracks::{RawSubtitle, RawSubtitleMap, audio_tracks, sub_langs, subtitle_tracks};
pub use variants::{synthesize_from_inventory, synthesize_variants};

/// Settings shared between the runner, the probe and the settings endpoint.
pub type SharedMediaSettings = Arc<RwLock<MediaSettings>>;

/// Reads the media settings from the `service.settings` blob.
///
/// Refuses a malformed blob rather than running on defaults: this is read once at start-up,
/// so an unusable configuration must stop the service instead of silently reverting the
/// download options the person configured.
pub async fn load_media_settings(database: &Database) -> Result<MediaSettings> {
    database.service_settings().await
}

/// Creates the shared settings handle from the database.
pub async fn shared_settings(database: &Database) -> Result<SharedMediaSettings> {
    Ok(Arc::new(RwLock::new(load_media_settings(database).await?)))
}

/// Convenience: runner + probe wired to the same settings handle.
pub fn build(
    database: Database,
    secrets: rd_secrets::SecretStore,
    settings: SharedMediaSettings,
) -> (Arc<dyn ExternalRunner>, Arc<dyn MediaProbe>) {
    (
        Arc::new(MediaRunner::new(database, secrets, settings.clone())),
        Arc::new(YtDlpProbe::new(settings)),
    )
}

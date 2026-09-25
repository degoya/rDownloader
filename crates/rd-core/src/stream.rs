//! Livestream recording via external `streamlink`: monitored channels and the tool
//! settings (part of the `service.settings` blob, keys prefixed `record_`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{CategoryId, StreamChannelId};

/// Provider of a link that is recorded rather than downloaded.
///
/// Assigned by the online check, not at intake: whether a manifest is live cannot be known
/// from its address, only from its body (RD-080-06). A candidate carrying it is enqueued as
/// [`crate::DownloadKind::Record`].
pub const RECORD_PROVIDER: &str = "record";

/// A channel watched by the monitor; recordings start automatically while it is live.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StreamChannel {
    pub id: StreamChannelId,
    /// Channel page URL handed to streamlink.
    pub url: String,
    /// Display name; doubles as the recording file prefix.
    pub name: String,
    /// streamlink stream selection (`best`, `1080p`, `720p`, …); `None` = the default.
    pub quality: Option<String>,
    /// Destination category of recordings; `None` = the default category.
    pub category_id: Option<CategoryId>,
    pub enabled: bool,
    pub last_live_at: Option<DateTime<Utc>>,
    /// Last probe error, cleared on a successful probe.
    pub last_error: Option<String>,
    /// Splitting, remux, sidecars and VOD fallback for this channel's recordings
    /// (RD-080-09).
    #[serde(default)]
    pub recording: crate::RecordingPolicy,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct StreamSettings {
    /// Absolute path of streamlink; `None` = vendor folders (incl. the portable build under
    /// `vendor/streamlink/bin`) and `PATH`.
    pub record_streamlink_executable: Option<String>,
    /// Stream selection used when a channel has none (`best`, `1080p`, …).
    pub record_default_quality: String,
    /// Seconds between liveness probes of enabled channels (60–3600).
    pub record_poll_interval_seconds: u32,
    /// Concurrent recordings (1–8); recordings never block regular downloads.
    pub record_max_parallel: u32,
    /// Shared with the other tools; searched before `PATH`.
    pub vendor_directory: Option<String>,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            record_streamlink_executable: None,
            record_default_quality: "best".to_owned(),
            record_poll_interval_seconds: 120,
            record_max_parallel: 2,
            vendor_directory: None,
        }
    }
}

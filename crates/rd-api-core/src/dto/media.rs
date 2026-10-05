//! External media tools, the managed tool store, stream channels and recordings.

use super::*;

/// Availability of one external tool.
#[derive(Serialize, ToSchema)]
pub struct MediaToolStatus {
    pub name: String,
    pub path: Option<String>,
    pub version: Option<String>,
    /// Whether the binary came from the explicit setting, the managed store, a vendor folder
    /// or `PATH`.
    pub source: Option<rd_core::ToolSource>,
    /// Whether this is a tool the application can manage itself (RD-102-02). A managed tool
    /// may still resolve to a system binary; that is what `source` says.
    pub managed: bool,
    /// The managed version currently activated, if any. Independent of `version`, which is
    /// whatever the resolved binary reports about itself.
    pub active_version: Option<String>,
    /// What the compatibility rules make of the version that was found (RD-102-03).
    pub compatibility: ToolCompatibility,
}

/// The compatibility verdict for one external tool (RD-102-03).
///
/// Four states rather than a boolean, because they call for different answers: `too_old` has
/// an upgrade, `known_bad` may call for a different version in either direction, and `unknown`
/// is an absence of information that never blocks anything.
#[derive(Serialize, ToSchema)]
pub struct ToolCompatibility {
    /// `supported`, `too_old`, `known_bad` or `unknown`.
    pub verdict: rd_tools::Verdict,
    /// The version the rules were applied to, normalised when it could be parsed and the raw
    /// line when it could not.
    pub version: Option<String>,
    /// The oldest version this build is tested against, when a rule sets one.
    pub min_version: Option<String>,
    /// What is lost while the verdict is not `supported`. Empty when no rule covers the tool.
    pub affects: Vec<rd_tools::Capability>,
    /// Whether the settings name this tool as overridden, so the verdict is reported but not
    /// enforced.
    pub overridden: bool,
    /// One English sentence saying what to do about it, or `null` when there is nothing to do.
    pub upgrade: Option<String>,
}

impl From<rd_tools::Assessment> for ToolCompatibility {
    fn from(assessment: rd_tools::Assessment) -> Self {
        Self {
            upgrade: assessment.upgrade(),
            verdict: assessment.verdict,
            version: assessment.version,
            min_version: assessment.min_version,
            affects: assessment.affects,
            overridden: assessment.overridden,
        }
    }
}

/// One managed external tool: what is installed, what is active, what is on offer.
#[derive(Serialize, ToSchema)]
pub struct ManagedToolInfo {
    /// `yt-dlp`, `gallery-dl`, `streamlink`, `ffmpeg` or `ffprobe`.
    pub name: String,
    /// The version the managed stage of the tool lookup answers with.
    pub active_version: Option<String>,
    /// The executable that version points at.
    pub active_path: Option<String>,
    /// Every version in the store, newest install first.
    pub installed_versions: Vec<String>,
    /// The newest version the signed manifest offers for this platform and this application
    /// version, or `null` when it offers none.
    pub available_version: Option<String>,
    /// Whether a rollback has an earlier installed version to return to.
    pub can_roll_back: bool,
}

/// The managed tool store as a whole.
#[derive(Serialize, ToSchema)]
pub struct ManagedToolsResponse {
    /// Whether this installation may install and activate tool versions at all.
    pub enabled: bool,
    /// The target triple manifest entries are matched against.
    pub platform: String,
    /// The sequence of the manifest currently in force. `0` is the manifest compiled into
    /// this build, before any refresh.
    pub manifest_sequence: u64,
    /// When the publisher signed that manifest.
    pub manifest_issued_at: String,
    /// The configured manifest URL, if any.
    pub manifest_url: Option<String>,
    pub tools: Vec<ManagedToolInfo>,
}

/// Which version to install or activate. `null` takes the newest the manifest offers.
#[derive(Deserialize, ToSchema)]
pub struct ManagedToolVersionRequest {
    #[serde(default)]
    pub version: Option<String>,
}

/// External-tool availability and the hosts routed to the media provider.
#[derive(Serialize, ToSchema)]
pub struct MediaStatusResponse {
    pub ytdlp: MediaToolStatus,
    pub ffmpeg: MediaToolStatus,
    /// yt-dlp needs ffprobe next to ffmpeg to merge streams and convert audio.
    pub ffprobe: MediaToolStatus,
    pub unrar: MediaToolStatus,
    pub seven_zip: MediaToolStatus,
    pub rclone: MediaToolStatus,
    pub gallery_dl: MediaToolStatus,
    pub streamlink: MediaToolStatus,
    /// The notification CLI; a target may name its own executable, this is the lookup without.
    pub apprise: MediaToolStatus,
    /// Vendor folders searched before `PATH`, in order.
    pub vendor_directories: Vec<String>,
    pub hosts: Vec<String>,
}

/// Editable fields of a monitored livestream channel.
#[derive(Deserialize, ToSchema)]
pub struct StreamChannelRequest {
    #[schema(format = "uri")]
    pub url: String,
    /// Display name; defaults to the URL host. Doubles as the recording file prefix.
    pub name: Option<String>,
    /// streamlink stream selection (`best`, `1080p`, …); `null` = the default quality.
    pub quality: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default = "default_channel_enabled")]
    pub enabled: bool,
    /// Splitting, remux, sidecars and reconnect delay for this channel (RD-080-09).
    #[serde(default)]
    pub recording: rd_core::RecordingPolicy,
}

/// Immediate one-off recording of a livestream URL.
#[derive(Deserialize, ToSchema)]
pub struct RecordNowRequest {
    #[schema(format = "uri")]
    pub url: String,
    pub name: Option<String>,
    pub quality: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
}

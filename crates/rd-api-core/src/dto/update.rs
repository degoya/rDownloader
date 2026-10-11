//! The application update status (RD-180-01): what runs, what is offered, what to do about it.

use super::*;

/// The update check's state, as `GET /api/v1/system/update` answers it.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct UpdateStatusResponse {
    /// The running version.
    pub current_version: String,
    /// Whether this build carries the update signing key. Without it nothing is checked, and
    /// the interface says so instead of reporting an error on every check.
    pub configured: bool,
    /// Whether the service checks by itself (`update_check_enabled`).
    pub check_enabled: bool,
    /// The chosen channel, `stable` or `beta` (`update_channel`).
    pub channel: String,
    /// The channel actually read: `stable` for an installation whose package manager publishes
    /// no pre-releases, whatever was chosen.
    pub effective_channel: String,
    /// Hours between two automatic checks.
    pub interval_hours: u32,
    /// Whether an offered update installs by itself (`update_auto_install`, RD-1240-27).
    #[serde(default)]
    pub auto_install: bool,
    /// Whether this installation installs an update itself (the portable archive, the Windows
    /// installer), and so whether `update_auto_install` can do anything; elsewhere a package
    /// manager or the container runtime updates it.
    #[serde(default)]
    pub installs_itself: bool,
    /// How this installation was installed: `portable`, `msi`, `deb`, `rpm`, `homebrew`,
    /// `scoop`, `winget`, `aur`, `docker` or `unknown`.
    pub install_kind: String,
    /// Whether a check is running right now.
    pub checking: bool,
    /// When the last check ran, RFC 3339.
    pub last_checked_at: Option<String>,
    /// When the next automatic check is due, RFC 3339; empty when none is scheduled.
    pub next_check_at: Option<String>,
    /// The stable code of what the last check refused or could not reach, if anything.
    pub error_code: Option<String>,
    /// The newer version this installation is offered, if any.
    pub available: Option<UpdateOffer>,
    /// Where an update this installation installs itself stands, or how the last one ended
    /// (RD-180-02); empty when there is none to tell about.
    pub install: Option<UpdateInstallStatus>,
    /// The background download of the offered version (RD-180-02): running, ready to install,
    /// or failed; empty when none was asked for since the start, or it is of another version.
    pub download: Option<UpdateDownloadStatus>,
    /// The capture agents connected right now, by the version each reported (RD-190-07); empty
    /// when none runs. Labels are not part of it: this status is readable with `api:read`.
    #[serde(default)]
    pub capture_agents: Vec<CaptureAgentVersion>,
}

/// One connected capture agent's version, measured against the service's.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
pub struct CaptureAgentVersion {
    /// The version the agent reported; empty for an agent from before 1.9, which reports none.
    pub version: Option<String>,
    /// Whether the agent is older than the service -- it reported an older version, or none --
    /// and so still runs the program file from before the update.
    pub outdated: bool,
    /// Where the agent's own update stands (RD-1210-03): `with_service` (the service in its
    /// folder updates it), `disabled`, `unchecked`, `current`, `offered`, `failed` or
    /// `installing`. Empty for an agent that reports none (before 1.21).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update: Option<String>,
    /// The newer version the agent offers itself, with `offered` and `installing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offered_version: Option<String>,
    /// Whether the agent lets the service ask it to install an update; off unless its own
    /// configuration allows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_update_allowed: Option<bool>,
}

/// The offered version's artifact, downloaded and verified in the background, which "Install and
/// restart" then installs without downloading it again.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct UpdateDownloadStatus {
    pub version: String,
    /// `downloading`, `ready` (on disk, size and SHA-256 those of the signed manifest) or
    /// `failed`.
    pub state: String,
    pub received_bytes: u64,
    pub total_bytes: u64,
    /// The stable code of why it failed, e.g. `update.digest_mismatch`.
    pub reason: Option<String>,
}

/// The self-update of RD-180-02, as the interface follows it through the restart.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct UpdateInstallStatus {
    /// `downloading`, `preparing` (the backup before the update), `restarting` (the updater
    /// stops the service), `installing`, `verifying` (the new version started and is asked for
    /// its version), `rolling_back`, `done`, `rolled_back` or `failed`.
    pub state: String,
    pub from_version: String,
    pub target_version: String,
    /// The stable code of why it failed or was rolled back, e.g. `update.health_timeout`.
    pub reason: Option<String>,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339.
    pub updated_at: String,
}

/// What `POST /api/v1/system/update/install` takes.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct UpdateInstallRequest {
    /// Install even while downloads run. The stop saves the queue and the downloads resume after
    /// the restart; without this the request is refused with `update.transfers_active`.
    #[serde(default)]
    pub allow_active: bool,
}

/// A newer version and how to get it.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct UpdateOffer {
    pub version: String,
    /// `stable` or `beta`.
    pub channel: String,
    /// RFC 3339.
    pub released_at: String,
    /// Short plain-text release notes for users, one `- ` point per line (RD-1150-02); render
    /// as text, never as markup.
    pub notes: String,
    /// The release page: downloads and checksums.
    pub release_url: String,
    /// The version's section of `CHANGELOG.md` at its tag: the full changes.
    pub changelog_url: String,
    /// `install` (this installation installs the artifact itself and restarts, RD-180-02),
    /// `download` (the artifact is replaced by hand) or `command` (a package manager or
    /// container runtime does it).
    pub action: String,
    /// The command to run, for `action` = `command`.
    pub command: Option<String>,
    /// A stable code the interface translates beside the command, e.g. `update.hint.docker_recreate`.
    pub hint: Option<String>,
    /// The artifact for this platform and installation kind, when the release has one.
    pub download_url: Option<String>,
    pub download_size: Option<u64>,
    pub download_sha256: Option<String>,
    /// For `install`: whether a new version that does not prove healthy is taken back by
    /// itself — always for the portable archive; for the Windows installer only when the last
    /// update kept the running version's installer, since a Windows Installer upgrade has no way
    /// back without the previous package.
    pub rollback_available: Option<bool>,
}

/// When an automatic install may start (`update_auto_install_window`, RD-1240-27): local times
/// in the installation's time zone (`bandwidth_timezone`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct UpdateInstallWindow {
    /// Minutes after midnight, inclusive (0-1439).
    pub start_minute: u16,
    /// Minutes after midnight, exclusive (0-1439); below `start_minute` the window wraps past
    /// midnight.
    pub end_minute: u16,
}

impl From<UpdateInstallWindow> for rd_update::InstallWindow {
    fn from(window: UpdateInstallWindow) -> Self {
        Self {
            start_minute: window.start_minute,
            end_minute: window.end_minute,
        }
    }
}

impl SettingsResponse {
    /// Refuses an update channel, interval, install window or backup retention the update cannot
    /// use (RD-180-01, RD-1240-27, RD-1240-34).
    pub fn validate_update(&self) -> Result<(), crate::ApiError> {
        if let Some(window) = self.update_auto_install_window
            && !rd_update::InstallWindow::from(window).is_valid()
        {
            return Err(crate::ApiError::bad_request(
                "settings.update_auto_install_window_invalid",
                "The install window needs a start and an end between 00:00 and 23:59 that differ",
            ));
        }
        if rd_update::Channel::parse(&self.update_channel).is_none() {
            return Err(crate::ApiError::bad_request(
                "settings.update_channel_invalid",
                "The update channel must be stable or beta",
            ));
        }
        let range = rd_update::settings::INTERVAL_HOURS_RANGE;
        if !range.contains(&self.update_check_interval_hours) {
            return Err(crate::ApiError::bad_request(
                "settings.update_interval_invalid",
                format!(
                    "The update check interval must be between {} and {} hours",
                    range.start(),
                    range.end()
                ),
            )
            .with_param("min", *range.start())
            .with_param("max", *range.end()));
        }
        let days = rd_update::settings::BACKUP_RETENTION_DAYS_RANGE;
        if !days.contains(&self.update_backup_retention_days) {
            return Err(crate::ApiError::bad_request(
                "settings.update_backup_retention_invalid",
                format!(
                    "The days the backup before an update is kept must be between {} and {}",
                    days.start(),
                    days.end()
                ),
            )
            .with_param("min", *days.start())
            .with_param("max", *days.end()));
        }
        Ok(())
    }
}

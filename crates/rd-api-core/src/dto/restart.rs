//! A pending restart and the restart itself (RD-1240-32): `GET` and `POST /api/v1/system/restart`.

use super::*;

/// Whether a restart is pending, why, and how this installation restarts.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct RestartStatusResponse {
    /// Something waits for the next start.
    pub pending: bool,
    /// What waits for it; empty when nothing does.
    pub reasons: Vec<RestartReason>,
    /// Whether `POST /api/v1/system/restart` is accepted now.
    pub can_restart: bool,
    /// `self` (rDownloader starts itself again), `supervisor` (systemd or the container runtime
    /// starts it again on exit code 75) or `manual` (it stops; whoever started it starts it).
    pub how: String,
    /// For `how` = `supervisor`: `systemd` or `container`. A container started without a
    /// restart policy stays stopped.
    pub supervisor: Option<String>,
    /// Why `can_restart` is false: `restart.update_running` or `restart.already_restarting`.
    pub blocked_reason: Option<String>,
    /// A restart was asked for and the service is on its way down.
    pub restarting: bool,
    /// Whether a pending restart happens by itself (`restart_when_needed`).
    pub automatic: bool,
    /// When this service process started (RFC 3339); a client that asked for a restart reads it
    /// changed once the service is back.
    pub started_at: String,
}

/// One thing that waits for the next start.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct RestartReason {
    /// `plugin_installed`, `plugin_updated`, `plugin_staged`, `plugin_unstaged`,
    /// `plugin_enabled`, `plugin_disabled`, `plugin_removed`, `plugin_key_revoked`,
    /// `plugin_digest_revoked` or `plugin_digest_unrevoked`.
    pub code: String,
    pub plugin_id: Option<String>,
    /// The plugin's name; for `plugin_key_revoked` the key's.
    pub name: Option<String>,
    /// The version that runs from the next start, or the one the reason is about.
    pub version: Option<String>,
    /// For `plugin_updated`: the version that runs now.
    pub from_version: Option<String>,
}

impl RestartReason {
    /// A reason about plugin `plugin_id`.
    #[must_use]
    pub fn plugin(code: &str, plugin_id: &str, name: Option<&str>, version: Option<&str>) -> Self {
        Self {
            code: code.to_owned(),
            plugin_id: Some(plugin_id.to_owned()),
            name: name.map(str::to_owned),
            version: version.map(str::to_owned),
            from_version: None,
        }
    }
}

/// Body of `POST /api/v1/system/restart`.
#[derive(Clone, Copy, Debug, Default, Deserialize, ToSchema)]
pub struct RestartRequest {
    /// Restart although downloads are running: the stop saves them, and they continue after
    /// the restart. Without it running downloads refuse the restart (`restart.transfers_active`).
    #[serde(default)]
    pub allow_active: bool,
}

/// What a restart that began answers.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct RestartStartedResponse {
    /// As in [`RestartStatusResponse::how`].
    pub how: String,
    pub supervisor: Option<String>,
}

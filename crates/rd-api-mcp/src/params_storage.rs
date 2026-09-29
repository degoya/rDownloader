//! Parameters of the collision, duplicate and storage-history tools (RD-150-01, RD-150-02)
//! and of the full backup's tools (RD-160-01, RD-160-02).

use rmcp::schemars;
use serde::Deserialize;

use crate::ApiError;

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CollisionPolicyParams {
    /// The category (id from list_configuration section categories) or the package (id from
    /// list_packages), depending on the tool.
    pub id: String,
    /// `rename`, `skip`, `overwrite`, `compare` or `ask`; absent removes the level's own policy
    /// so it inherits again.
    #[serde(default)]
    pub policy: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CollisionDecisionParams {
    /// The waiting download, as list_collision_prompts names it.
    pub id: String,
    /// `rename`, `skip` or `overwrite`.
    pub decision: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DuplicateLookupParams {
    /// Addresses to look up (at most 500).
    pub urls: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DedupeParams {
    /// The finished download whose file becomes a link.
    pub id: String,
    /// The finished download whose identical file stays.
    pub original_download_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct StorageOperationsParams {
    /// Newest rows to return (1-1000, default 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// A policy word, refused with the code the interface translates.
pub(crate) fn policy(value: Option<&str>) -> Result<Option<rd_core::CollisionPolicy>, ApiError> {
    value
        .map(|value| {
            rd_core::CollisionPolicy::parse(value).ok_or_else(|| {
                ApiError::bad_request(
                    "collision.policy_invalid",
                    "The policy must be rename, skip, overwrite, compare or ask",
                )
            })
        })
        .transpose()
}

/// A decision word, refused with the code the interface translates.
pub(crate) fn decision(value: &str) -> Result<rd_core::CollisionDecision, ApiError> {
    rd_core::CollisionDecision::parse(value).ok_or_else(|| {
        ApiError::bad_request(
            "collision.decision_invalid",
            "The decision must be rename, skip or overwrite",
        )
    })
}

/// The full backup's schedule (RD-160-01) and its verification schedule (RD-160-02). The
/// passphrase is not a parameter of any tool: it is set up in the interface.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct BackupScheduleParams {
    /// Whether the schedule runs. Switching it on needs a passphrase set up in the interface
    /// and at least one destination (create_backup_destination).
    pub enabled: bool,
    /// Five-field cron expression (minute hour day month weekday), e.g. `0 3 * * *`.
    pub schedule: String,
    /// IANA time zone the schedules are read in, e.g. `Europe/Berlin`.
    pub timezone: String,
    /// Five-field cron expression of the scheduled verification of the newest archive at every
    /// destination; empty or absent switches it off.
    #[serde(default)]
    pub verify_schedule: Option<String>,
}

/// A backup destination (RD-160-02). No credential is a parameter: an object storage profile is
/// named by its id (list_configuration), an rclone remote by its name.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct BackupDestinationParams {
    /// `local` (a folder or mounted NAS share), `object_storage` or `rclone` (WebDAV too).
    pub kind: String,
    /// How the interface names it; absent is the destination's own address.
    #[serde(default)]
    pub name: Option<String>,
    /// Absent is on.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// `local`: an absolute folder on the machine running rDownloader.
    #[serde(default)]
    pub path: Option<String>,
    /// `object_storage`: the profile's id.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// `object_storage`: `<bucket>/<folder>`; an empty bucket is the profile's bound one.
    #[serde(default)]
    pub prefix: Option<String>,
    /// `rclone`: `name:path` of a remote configured in rclone.
    #[serde(default)]
    pub remote: Option<String>,
    /// Keep the newest this many archives there; absent is no limit by count.
    #[serde(default)]
    pub keep_last: Option<u32>,
    /// Keep archives this many days; absent is no limit by age. The newest always stays.
    #[serde(default)]
    pub keep_days: Option<u32>,
}

/// A destination to replace, by id, with all its fields.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct BackupDestinationUpdateParams {
    /// The destination's id from get_backup_status.
    pub id: String,
    #[serde(flatten)]
    pub destination: BackupDestinationParams,
}

/// One backup destination or archive by id.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct BackupIdParams {
    /// The id from get_backup_status (a destination) or list_backup_archives (an archive).
    pub id: String,
}

/// A retention preview of one destination.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct BackupRetentionParams {
    /// The destination's id from get_backup_status.
    pub id: String,
    /// Preview this count instead of the stored one.
    #[serde(default)]
    pub keep_last: Option<u32>,
    /// Preview this age in days instead of the stored one.
    #[serde(default)]
    pub keep_days: Option<u32>,
}

/// The archive ledger, of one destination or of all.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct BackupArchivesParams {
    /// Only this destination's archives; absent lists every destination's.
    #[serde(default)]
    pub destination_id: Option<String>,
}

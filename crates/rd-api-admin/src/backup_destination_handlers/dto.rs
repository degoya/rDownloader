//! What the destination, archive and verification routes take and answer.

use super::*;

/// A destination as the interface shows it.
#[derive(Serialize, ToSchema)]
pub struct BackupDestinationResponse {
    pub id: String,
    /// `local`, `object_storage` or `rclone`.
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    /// The folder of a `local` destination.
    pub path: Option<String>,
    /// The object storage profile of an `object_storage` destination.
    pub profile_id: Option<String>,
    /// `<bucket>/<folder>` of an `object_storage` destination; an empty bucket is the
    /// profile's bound one.
    pub prefix: Option<String>,
    /// `name:path` of an `rclone` destination.
    pub remote: Option<String>,
    /// The newest archives kept; `None` is no limit by count.
    pub keep_last: Option<u32>,
    /// The days archives are kept; `None` is no limit by age.
    pub keep_days: Option<u32>,
    /// Archives this installation recorded there.
    pub archive_count: u32,
    pub last_stored_at: Option<DateTime<Utc>>,
    /// The last verification of the newest archive there.
    pub last_verify_state: Option<BackupVerifyState>,
}

/// A destination to create or replace. Only the fields of its kind are read.
#[derive(Deserialize, ToSchema)]
pub struct BackupDestinationRequest {
    /// `local`, `object_storage` or `rclone`.
    pub kind: String,
    /// How the interface and the history name it; empty is the destination's own address.
    #[serde(default)]
    pub name: Option<String>,
    /// Missing is on.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// `local`: an absolute folder, checked like a storage root and created when missing.
    #[serde(default)]
    pub path: Option<String>,
    /// `object_storage`: the profile's id.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// `object_storage`: `<bucket>/<folder>`.
    #[serde(default)]
    pub prefix: Option<String>,
    /// `rclone`: `name:path` of a configured remote.
    #[serde(default)]
    pub remote: Option<String>,
    /// At least 1; missing is no limit by count.
    #[serde(default)]
    pub keep_last: Option<u32>,
    /// At least 1; missing is no limit by age.
    #[serde(default)]
    pub keep_days: Option<u32>,
}

/// One archive of the ledger.
#[derive(Clone, Serialize, ToSchema)]
pub struct BackupArchiveResponse {
    pub id: String,
    pub destination_id: String,
    pub run_id: String,
    pub archive_name: String,
    pub location: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
    pub stored_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,
    pub verify_state: Option<BackupVerifyState>,
    pub verify_code: Option<String>,
}

impl From<&BackupArchive> for BackupArchiveResponse {
    fn from(archive: &BackupArchive) -> Self {
        Self {
            id: archive.id.clone(),
            destination_id: archive.destination_id.clone(),
            run_id: archive.run_id.clone(),
            archive_name: archive.archive_name.clone(),
            location: archive.location.clone(),
            size_bytes: archive.size_bytes,
            sha256: archive.sha256.clone(),
            created_at: archive.created_at,
            stored_at: archive.stored_at,
            verified_at: archive.verified_at,
            verify_state: archive.verify_state,
            verify_code: archive.verify_code.clone(),
        }
    }
}

/// What a retention pass would do at a destination; nothing is deleted by asking.
#[derive(Serialize, ToSchema)]
pub struct RetentionPreviewResponse {
    pub keep_last: Option<u32>,
    pub keep_days: Option<u32>,
    /// Newest first.
    pub keep: Vec<BackupArchiveResponse>,
    pub remove: Vec<BackupArchiveResponse>,
}

/// A policy to preview instead of the stored one, for a form that is being edited.
#[derive(Debug, Deserialize)]
pub struct RetentionPreviewQuery {
    #[serde(default)]
    pub keep_last: Option<u32>,
    #[serde(default)]
    pub keep_days: Option<u32>,
}

/// Filters the ledger to one destination.
#[derive(Debug, Deserialize)]
pub struct ArchiveQuery {
    #[serde(default)]
    pub destination_id: Option<String>,
}

/// One verification of one archive.
#[derive(Serialize, ToSchema)]
pub struct BackupVerificationResponse {
    pub id: String,
    pub origin: BackupOrigin,
    pub state: BackupVerifyState,
    pub archive_id: Option<String>,
    pub destination_id: Option<String>,
    pub destination: String,
    pub archive_name: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Whether the archive was opened and every member checked, or only its size and SHA-256
    /// compared (an archive sealed under an earlier passphrase).
    pub content_checked: Option<bool>,
    /// `backup.verify_missing`, `backup.verify_digest_mismatch`, `backup.verify_damaged` or
    /// the destination's own code.
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
}

impl From<BackupVerification> for BackupVerificationResponse {
    fn from(row: BackupVerification) -> Self {
        Self {
            id: row.id,
            origin: row.origin,
            state: row.state,
            archive_id: row.archive_id,
            destination_id: row.destination_id,
            destination: row.destination,
            archive_name: row.archive_name,
            started_at: row.started_at,
            finished_at: row.finished_at,
            content_checked: row.content_checked,
            error_code: row.error_code,
            error_detail: row.error_detail,
        }
    }
}

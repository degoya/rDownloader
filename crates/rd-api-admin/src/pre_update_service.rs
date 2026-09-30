//! The backup the updater asks for before it switches versions (RD-180-03): what
//! `POST /api/v1/system/update/prepare` does.
//!
//! Two parts, from `rd_backup::pre_update`: always the checked database copy, and — when a
//! backup passphrase is set up — the encrypted full backup of RD-160-01 under that key, opened
//! again and read to its end. Both land in `<data directory>/pre-update/` with their own
//! retention of three, apart from the scheduled backup's destinations. The database copy failing
//! refuses the update (`update.backup_failed`); so does the encrypted backup failing when the
//! release changes the schema, because then the old version cannot open what the new one
//! leaves. Without a passphrase there is no encrypted backup to write, and the answer says so
//! (`backup.key_missing`) rather than refusing: the copy alone is what the rollback of the
//! database needs. A refusal sweeps whatever the attempt left, so no half copy stays.
//!
//! One preparation at a time per process; a second one meanwhile is `update.prepare_running`.

use std::sync::LazyLock;

use rd_backup::pre_update::{self, UpdatePlan};
use rd_core::AuditAction;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::audit::{AuditContext, AuditEvent};
use crate::{ApiError, AppState};

/// Held for the length of one preparation.
static PREPARING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// What the updater asks for.
#[derive(Deserialize, ToSchema)]
pub struct PreUpdateRequest {
    /// The version the updater is about to switch to; letters, digits, `.`, `_`, `+` and `-`.
    pub target_version: String,
    /// Whether that version changes the database schema, from the release manifest. When it
    /// does, the encrypted backup is required as well, if a passphrase is set up.
    #[serde(default)]
    pub schema_change: bool,
}

/// The checked database copy.
#[derive(Serialize, ToSchema)]
pub struct PreUpdateCopy {
    pub path: String,
    pub size_bytes: u64,
    /// The highest migration the copy has applied: the schema the running version needs.
    pub schema_version: i64,
    pub packages: u64,
    pub downloads: u64,
    pub unfinished: u64,
    pub storage_roots: u64,
    pub categories: u64,
    pub accounts: u64,
}

/// The checked encrypted full backup.
#[derive(Serialize, ToSchema)]
pub struct PreUpdateArchive {
    pub path: String,
    pub archive_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    /// The fingerprint of the key it is sealed under; restoring it needs that passphrase.
    pub key_fingerprint: String,
}

/// What was written before the update, all of it checked.
#[derive(Serialize, ToSchema)]
pub struct PreUpdateResponse {
    /// The version running now, which the copy and the archive belong to.
    pub from_version: String,
    pub target_version: String,
    pub schema_change: bool,
    /// `<data directory>/pre-update`.
    pub directory: String,
    pub database_copy: PreUpdateCopy,
    pub encrypted_backup: Option<PreUpdateArchive>,
    /// Why there is no encrypted backup: `backup.key_missing`, or the stable code of the step
    /// that failed when the release changes no schema and the update may go ahead without it.
    pub encrypted_backup_code: Option<String>,
}

/// Writes and checks the backup before an update.
///
/// # Errors
///
/// `update.target_version_invalid` for a version that is no file name, `update.prepare_running`
/// while another preparation runs, `update.backup_failed` when the update must not start.
pub async fn prepare(
    state: &AppState,
    request: PreUpdateRequest,
    audit: &AuditContext,
) -> Result<PreUpdateResponse, ApiError> {
    let target = request.target_version.trim().to_owned();
    if !pre_update::is_version(&target) {
        return Err(ApiError::bad_request(
            "update.target_version_invalid",
            "The target version may hold only letters, digits, '.', '_', '+' and '-'",
        ));
    }
    let Ok(_preparing) = PREPARING.try_lock() else {
        return Err(ApiError::conflict(
            "update.prepare_running",
            "A backup before an update is already being written",
        ));
    };
    let data = crate::backup_service::data_directory(state);
    let from = env!("CARGO_PKG_VERSION");
    let plan = UpdatePlan {
        data_directory: &data,
        from_version: from,
        target_version: &target,
        at: chrono::Utc::now(),
    };
    sweep(&data).await;
    let event = AuditEvent::success(AuditAction::UpdatePrepared)
        .by(audit)
        .target("update", &target)
        .detail("from_version", from)
        .detail("schema_change", request.schema_change);

    let copy = match pre_update::copy_database(&state.database, plan).await {
        Ok(copy) => copy,
        Err(error) => return Err(refuse(state, &data, event, "database_copy", error).await),
    };
    let (encrypted_backup, encrypted_backup_code) = match encrypted(state, plan).await {
        Ok(Some(archive)) => (Some(archive), None),
        Ok(None) => (None, Some("backup.key_missing".to_owned())),
        Err(error) if request.schema_change => {
            return Err(refuse(state, &data, event, "encrypted_backup", error).await);
        }
        Err(error) => {
            tracing::warn!(
                code = error.code,
                detail = %error.detail,
                "the encrypted backup before the update failed; the release changes no \
                 schema, so the checked database copy is enough"
            );
            sweep(&data).await;
            (None, Some(error.code.to_owned()))
        }
    };
    tracing::info!(
        copy = %copy.path.display(),
        encrypted = encrypted_backup.is_some(),
        target = %target,
        "backup before the update written and checked"
    );
    let mut event = event.detail("copy", copy.path.display());
    if let Some(code) = &encrypted_backup_code {
        event = event.detail("encrypted_backup", code);
    }
    crate::audit::record(state, event).await;
    Ok(PreUpdateResponse {
        from_version: from.to_owned(),
        target_version: target,
        schema_change: request.schema_change,
        directory: pre_update::directory(&data).display().to_string(),
        database_copy: PreUpdateCopy {
            path: copy.path.display().to_string(),
            size_bytes: copy.size_bytes,
            schema_version: copy.schema_version,
            packages: copy.counts.packages,
            downloads: copy.counts.downloads,
            unfinished: copy.counts.unfinished,
            storage_roots: copy.counts.storage_roots,
            categories: copy.counts.categories,
            accounts: copy.counts.accounts,
        },
        encrypted_backup: encrypted_backup.map(|archive| PreUpdateArchive {
            path: archive.path.display().to_string(),
            archive_name: archive.archive_name,
            size_bytes: archive.size_bytes,
            sha256: archive.sha256,
            key_fingerprint: archive.key_fingerprint,
        }),
        encrypted_backup_code,
    })
}

/// The encrypted full backup, when a passphrase is set up; `None` when none is.
async fn encrypted(
    state: &AppState,
    plan: UpdatePlan<'_>,
) -> Result<Option<pre_update::VerifiedArchive>, rd_backup::BackupError> {
    let config = state
        .database
        .backup_config()
        .await
        .map_err(|error| rd_backup::BackupError {
            code: "backup.key_unavailable",
            detail: format!("{error:#}"),
        })?;
    let Some(record) = config.key.as_ref() else {
        return Ok(None);
    };
    let key = crate::backup_service::load_key(state, record).await?;
    let sources = crate::backup_service::backup_sources(state, &key, &config.instance_id).await?;
    pre_update::seal_archive(&state.database, sources, &key, plan)
        .await
        .map(Some)
}

/// Sweeps what the attempt left, audits the refusal and says what to do.
async fn refuse(
    state: &AppState,
    data: &std::path::Path,
    event: AuditEvent,
    stage: &'static str,
    error: rd_backup::BackupError,
) -> ApiError {
    sweep(data).await;
    tracing::error!(stage, code = error.code, detail = %error.detail, "the backup before an update failed; the update must not start");
    let mut failed = event.detail("stage", stage).detail("code", error.code);
    failed.outcome = rd_core::AuditOutcome::Failure;
    crate::audit::record(state, failed).await;
    ApiError::conflict(
        "update.backup_failed",
        format!(
            "The update must not start: the {} before it could not be written and checked \
             ({}: {}). The installed version keeps running unchanged. Remove the cause (free \
             space in {}, a backup passphrase that opens its key) and run the update again.",
            stage.replace('_', " "),
            error.code,
            error.detail,
            pre_update::directory(data).display()
        ),
    )
    .with_param("stage", stage)
    .with_param("cause", error.code)
}

async fn sweep(data: &std::path::Path) {
    if let Err(error) = pre_update::sweep(data).await {
        tracing::warn!(%error, "the pre-update folder could not be swept");
    }
}

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
//! Then the plugins (RD-1260-02): for each installed plugin the new version's contract would
//! refuse, the newest package the repositories offer for that contract is fetched into the
//! repositories' preparation folder, which the new version installs from at its start before it
//! reads the bundle. Nothing here refuses the update: a plugin without a package is named in the
//! answer, and the bundle stays its fallback.
//!
//! One preparation at a time per process; a second one meanwhile is `update.prepare_running`.

use std::sync::LazyLock;

use rd_backup::pre_update::{self, UpdatePlan};
use rd_core::{AuditAction, PluginId};
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
    /// The `rdownloader:plugin` contracts that version speaks, from the release manifest
    /// (RD-1260-02). Absent, the stored update offer's for the same version are taken; without
    /// either, no plugin is fetched.
    #[serde(default)]
    pub plugin_contracts: Option<Vec<String>>,
}

/// Most contracts one request may name.
const MAX_PLUGIN_CONTRACTS: usize = 16;

/// One plugin fetched for the new version.
#[derive(Serialize, ToSchema)]
pub struct PreUpdatePlugin {
    pub plugin_id: PluginId,
    pub name: String,
    pub version: String,
    pub repository_id: String,
}

/// The plugins fetched for the version the update installs (RD-1260-02).
#[derive(Serialize, ToSchema)]
pub struct PreUpdatePlugins {
    /// Waiting in the preparation folder; the new version installs them at its start, before it
    /// reads the bundle.
    pub prepared: Vec<PreUpdatePlugin>,
    /// Installed plugins the new version refuses that no repository had a package for, or whose
    /// package could not be fetched: the bundle replaces them at the next start where it can.
    pub missing: Vec<String>,
    /// Why nothing was fetched, `null` when nothing stood in the way:
    /// `update.plugin_contracts_unknown` when neither the request nor the release manifest names
    /// the new version's contracts, or the repository's code when the preparation failed.
    pub code: Option<String>,
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
    pub plugins: PreUpdatePlugins,
}

/// Writes and checks the backup before an update.
///
/// # Errors
///
/// `update.target_version_invalid` for a version that is no file name,
/// `update.plugin_contracts_invalid` for contracts that are no versions, `update.prepare_running`
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
    if let Some(contracts) = &request.plugin_contracts
        && (contracts.len() > MAX_PLUGIN_CONTRACTS
            || contracts.iter().any(|contract| !plain_contract(contract)))
    {
        return Err(ApiError::bad_request(
            "update.plugin_contracts_invalid",
            format!("Name at most {MAX_PLUGIN_CONTRACTS} plugin contracts, each a version"),
        )
        .with_param("max", MAX_PLUGIN_CONTRACTS));
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
    let plugins = prepare_plugins(state, &target, request.plugin_contracts).await;
    let mut event = event
        .detail("copy", copy.path.display())
        .detail("plugins_prepared", plugins.prepared.len());
    if let Some(code) = &encrypted_backup_code {
        event = event.detail("encrypted_backup", code);
    }
    if let Some(code) = &plugins.code {
        event = event.detail("plugins", code);
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
        plugins,
    })
}

/// `major.minor.patch` in digits, as a contract version is written.
fn plain_contract(contract: &str) -> bool {
    contract.len() <= 32
        && contract.split('.').count() == 3
        && contract
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Fetches the plugins the target version needs; see the module documentation.
async fn prepare_plugins(
    state: &AppState,
    target: &str,
    contracts: Option<Vec<String>>,
) -> PreUpdatePlugins {
    let contracts = match contracts.filter(|contracts| !contracts.is_empty()) {
        Some(contracts) => contracts,
        None => state
            .updates
            .stored()
            .await
            .offer
            .filter(|offer| offer.version == target)
            .map(|offer| offer.plugin_contracts)
            .unwrap_or_default(),
    };
    let mut answer = PreUpdatePlugins {
        prepared: Vec::new(),
        missing: Vec::new(),
        code: None,
    };
    if contracts.is_empty() {
        answer.code = Some("update.plugin_contracts_unknown".to_owned());
        return answer;
    }
    let outcome = state
        .plugin_repositories
        .prepare_update(rd_plugin_host::repository::UpdateTarget {
            version: target,
            contracts: &contracts,
        })
        .await;
    match outcome {
        Ok(outcome) => {
            if !outcome.prepared.is_empty() || !outcome.missing.is_empty() {
                tracing::info!(
                    prepared = outcome.prepared.len(),
                    missing = ?outcome.missing,
                    version = %target,
                    "plugins for the update's plugin contract fetched"
                );
            }
            answer.prepared = outcome
                .prepared
                .into_iter()
                .map(|plugin| PreUpdatePlugin {
                    plugin_id: plugin.plugin_id,
                    name: plugin.name,
                    version: plugin.version,
                    repository_id: plugin.repository_id,
                })
                .collect();
            answer.missing = outcome.missing;
        }
        Err(error) => {
            tracing::warn!(code = error.code(), %error, "the plugins for the update could not be fetched; the bundle stays their fallback");
            answer.code = Some(error.code().to_owned());
        }
    }
    answer
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

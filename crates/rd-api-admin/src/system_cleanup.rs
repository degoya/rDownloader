//! Cleaning up what updates and the plugin compile cache leave in the data directory
//! (RD-1240-34): `GET /api/v1/system/cleanup` says what a clean-up would remove, `POST` does it,
//! and every start does it once the service has run for [`START_DELAY`].
//!
//! Two stores. The copies kept for taking an update back (`rd_backup::update_retention`): behind
//! a proven update only the newest of each kind, and that one too after
//! `update_backup_retention_days`; an update not proven keeps all of them. And the compiled
//! plugin code (`rd_plugin_host::prune_compile_cache`) that no installed plugin uses, that an
//! older Wasmtime wrote, or that lies beyond the cache's cap. The managed tool store needs nothing
//! here: an activation already keeps only the active version and the two before it
//! (`rd_tools::store::KEPT_VERSIONS_PER_TOOL`).
//!
//! And the database (RD-1240-35, [`database`]): old skipped or dismissed subscription items keep
//! only their key, and the free pages go back to the file system.
//!
//! The start's pass waits: by then the start's compiles are recorded, an update the updater
//! proves has been proven, and a version that does not stay up has not removed the copies its
//! rollback needs. It repeats once a day, for the archive and the event retention's pages. It is
//! logged, not audited — nobody asked for it.

use std::time::{Duration, SystemTime};

use axum::{Json, extract::State};
use rd_backup::update_retention::{self, BackupKind, UpdateBackupPlan, UpdateBackupPolicy};
use rd_core::AuditAction;
use serde::Serialize;
use utoipa::ToSchema;

use crate::audit::{AuditContext, AuditEvent};
use crate::data_reset_handlers::{DataClearRequest, confirm};
use crate::{ApiError, AppState};

#[path = "system_cleanup_database.rs"]
mod database;
use database::Pass;
pub use database::{DatabaseCleanup, REWRITE_BUSY, REWRITE_FAILED, REWRITE_NO_SPACE};

/// How long a start runs before its clean-up.
pub const START_DELAY: Duration = Duration::from_secs(10 * 60);
/// How long the service waits between two clean-ups after that.
pub const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// One store: what stays and what goes.
#[derive(Clone, Copy, Debug, Default, Serialize, ToSchema)]
pub struct CleanupArea {
    pub kept_files: u64,
    pub kept_bytes: u64,
    /// In the preview what a clean-up would remove; in its answer what it removed.
    pub removable_files: u64,
    pub removable_bytes: u64,
}

/// The stores a clean-up looks at.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CleanupSummary {
    /// Whether the last update is proven, or none is recorded. Only then do the copies before
    /// updates and migrations thin out.
    pub update_proven: bool,
    /// `update_backup_retention_days`: how long the newest copy stays behind a proven update;
    /// 0 for good.
    pub retention_days: u32,
    /// Database copies and encrypted archives taken before updates.
    pub pre_update: CleanupArea,
    /// Database copies taken before migrations.
    pub pre_migration: CleanupArea,
    /// Compiled plugin code.
    pub plugin_cache: CleanupArea,
    /// The database file: its events, the subscription archive and its free pages.
    pub database: DatabaseCleanup,
}

impl CleanupSummary {
    /// Bytes a clean-up removes (or removed) across the stores of files; the database's
    /// shrinking is `database.removable_bytes`.
    #[must_use]
    pub fn removable_bytes(&self) -> u64 {
        self.pre_update.removable_bytes
            + self.pre_migration.removable_bytes
            + self.plugin_cache.removable_bytes
    }
}

/// What a clean-up would remove now. Nothing changes.
#[utoipa::path(
    get,
    path = "/api/v1/system/cleanup",
    tag = "system",
    responses((status = 200, body = CleanupSummary))
)]
pub async fn cleanup_preview(
    State(state): State<AppState>,
) -> Result<Json<CleanupSummary>, ApiError> {
    Ok(Json(clean_up(&state, Pass::Preview).await?))
}

/// Removes the old copies before updates and migrations and the compiled plugin code nothing
/// uses, compacts the old skipped or dismissed subscription items and shrinks the database
/// file — rewriting it once if it was created before 1.24, unless something downloads
/// (`system.cleanup_rewrite_busy`) or the data directory has not room for a second copy
/// (`system.cleanup_rewrite_no_space`); answers what stayed and what went.
#[utoipa::path(
    post,
    path = "/api/v1/system/cleanup",
    tag = "system",
    request_body = DataClearRequest,
    responses(
        (status = 200, body = CleanupSummary),
        (status = 400, description = "data_reset.not_confirmed"),
    )
)]
pub async fn run_cleanup(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<DataClearRequest>,
) -> Result<Json<CleanupSummary>, ApiError> {
    confirm(&request, "cleanup")?;
    let summary = clean_up(&state, Pass::Requested).await?;
    crate::audit::record(
        &state,
        AuditEvent::success(AuditAction::SystemCleanup)
            .by(&audit)
            .target("system", "data_directory")
            .detail("removed_bytes", summary.removable_bytes())
            .detail("pre_update_files", summary.pre_update.removable_files)
            .detail("pre_migration_files", summary.pre_migration.removable_files)
            .detail("plugin_cache_entries", summary.plugin_cache.removable_files)
            .detail("compacted_items", summary.database.compactable_items)
            .detail("database_bytes", summary.database.removable_bytes),
    )
    .await;
    Ok(Json(summary))
}

/// Starts the start's pass, [`START_DELAY`] from now, and one every [`INTERVAL`] after it.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(START_DELAY).await;
        loop {
            match clean_up(&state, Pass::Automatic).await {
                Ok(summary)
                    if summary.removable_bytes() > 0
                        || summary.database.removable_bytes > 0
                        || summary.database.compactable_items > 0 =>
                {
                    tracing::info!(
                        removed_bytes = summary.removable_bytes(),
                        pre_update = summary.pre_update.removable_files,
                        pre_migration = summary.pre_migration.removable_files,
                        plugin_cache = summary.plugin_cache.removable_files,
                        compacted_items = summary.database.compactable_items,
                        database_bytes = summary.database.removable_bytes,
                        "old update backups, unused compiled plugin code and database pages removed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error.message(), "the data directory was not cleaned up; the next pass tries again");
                }
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// One pass over every store; [`Pass::Preview`] only measures.
async fn clean_up(state: &AppState, pass: Pass) -> Result<CleanupSummary, ApiError> {
    let dry_run = pass == Pass::Preview;
    // First, so a plugin folder that cannot be read ends the pass before it removed anything:
    // pruned by an empty list, the cache would lose every installed plugin's code.
    let installed = state.plugins.installed_component_digests().await?;
    let data = crate::backup_service::data_directory(state);
    let settings: rd_update::UpdateSettings = state.database.service_settings_or_default().await?;
    let retention_days = settings.backup_retention_days();
    let update_proven =
        rd_update::install::recover::update_proven(&data, env!("CARGO_PKG_VERSION"));
    let policy = UpdateBackupPolicy {
        proven: update_proven,
        grace_days: retention_days,
        now: SystemTime::now(),
    };
    let backups = if dry_run {
        update_retention::plan(&data, policy).await
    } else {
        update_retention::apply(&data, policy).await
    };
    let cache = tokio::task::spawn_blocking(move || {
        rd_plugin_host::prune_compile_cache(&installed, dry_run)
    })
    .await
    .map_err(anyhow::Error::from)?;
    let database = database::clean_up(state, &data, pass).await?;
    Ok(CleanupSummary {
        update_proven,
        retention_days,
        pre_update: area(
            &backups,
            &[BackupKind::PreUpdateCopy, BackupKind::PreUpdateArchive],
        ),
        pre_migration: area(&backups, &[BackupKind::PreMigrationCopy]),
        plugin_cache: CleanupArea {
            kept_files: cache.kept_entries,
            kept_bytes: cache.kept_bytes,
            removable_files: cache.removed_entries,
            removable_bytes: cache.removed_bytes,
        },
        database,
    })
}

fn area(plan: &UpdateBackupPlan, kinds: &[BackupKind]) -> CleanupArea {
    let (kept_files, kept_bytes) = plan.kept_of(kinds);
    let (removable_files, removable_bytes) = plan.removable_of(kinds);
    CleanupArea {
        kept_files,
        kept_bytes,
        removable_files,
        removable_bytes,
    }
}

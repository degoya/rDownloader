//! The restore's REST surface (RD-160-03): preview, test restore, restore, and where a restore
//! stands. Every route costs `api:admin` (`scope_policy`); the uploads are `restore_uploads`.
//! The restore itself replaces the password hash, the passkeys and the tokens at the next start,
//! so it takes a signed-in session and the password again as well (RD-1190-19,
//! `rd_api_core::step_up::require_confirmed`); a bearer token is refused whatever its areas.
//!
//! Not offered through MCP: every step takes the passphrase in, which the owner's line of
//! 2026-09-23 keeps out of the toolbox (`mcp_coverage`), and the restore itself replaces the
//! whole installation at the next start.

use std::collections::BTreeMap;

use axum::{Json, extract::State, http::StatusCode};
use rd_backup::manifest::{PARTIAL_TRANSFERS_PART, PLUGIN_TRUST_PART, PartKind, SETTINGS_PART};
use rd_backup::restore::cutover::{self, Phase};
use rd_backup::restore::inspect;
use rd_core::AuditAction;

use crate::audit::{AuditContext, AuditEvent};
use crate::restore_dto::{
    RestorePartGroupResponse, RestorePathResponse, RestorePreviewRequest, RestorePreviewResponse,
    RestoreReportResponse, RestoreRequest, RestoreRootResponse, RestoreSourceRequest,
    RestoreStagedResponse, RestoreStatusResponse,
};
use crate::restore_service::{self, Busy, Mode, api_error, layout, native};
use crate::settings_backup_dto::SettingsBundle;
use crate::{ApiError, AppState};

/// How many other paths a preview lists.
const PATHS_LISTED: usize = 50;

fn keep_small_parts(part: &rd_backup::ManifestPart) -> bool {
    matches!(
        part.kind,
        PartKind::Settings | PartKind::PluginTrust | PartKind::PartialTransfers
    )
}

/// Whether version `left` is newer than `right`, by their numeric `major.minor.patch` heads.
fn newer(left: &str, right: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split(['.', '-', '+'])
            .take(3)
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parts(left) > parts(right)
}

/// The preview, for the route: the archive's manifest and small parts, nothing written.
pub async fn preview(
    state: &AppState,
    audit: &AuditContext,
    source: &RestoreSourceRequest,
    passphrase: &str,
) -> Result<RestorePreviewResponse, ApiError> {
    let archive = restore_service::resolve_source(state, source).await?;
    let key = restore_service::open(state, audit, &archive, passphrase, "preview").await?;
    let contents = inspect::read_archive(&archive, key, keep_small_parts)
        .await
        .map_err(api_error)?;
    let manifest = contents.manifest;
    let archive_size = tokio::fs::metadata(&archive)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or_default();

    let mut groups: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for part in &manifest.parts {
        let kind = serde_json::to_value(part.kind)
            .ok()
            .and_then(|kind| kind.as_str().map(str::to_owned))
            .unwrap_or_default();
        let entry = groups.entry(kind).or_default();
        entry.0 += 1;
        entry.1 += part.size;
    }

    let bundle: Option<SettingsBundle> = contents
        .kept
        .get(SETTINGS_PART)
        .and_then(|bytes| serde_json::from_slice(bytes).ok());
    let json = |name: &str| -> serde_json::Value {
        contents
            .kept
            .get(name)
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or_default()
    };
    let trust = json(PLUGIN_TRUST_PART);
    let plugin_trust_rows: usize = trust["tables"]
        .as_object()
        .map(|tables| {
            tables
                .values()
                .filter_map(serde_json::Value::as_array)
                .map(Vec::len)
                .sum::<usize>()
        })
        .unwrap_or_default();
    let partial = json(PARTIAL_TRANSFERS_PART);
    let downloads = partial["downloads"].as_array().cloned().unwrap_or_default();

    let mut paths = Vec::new();
    if let Some(bundle) = &bundle {
        for hotfolder in &bundle.hotfolders {
            paths.push(RestorePathResponse {
                kind: "hotfolder".to_owned(),
                native: native(&hotfolder.path),
                path: hotfolder.path.clone(),
            });
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for download in &downloads {
        if let Some(destination) = download["destination"].as_str()
            && seen.insert(destination.to_owned())
        {
            paths.push(RestorePathResponse {
                kind: "partial_transfer".to_owned(),
                native: native(destination),
                path: destination.to_owned(),
            });
        }
    }
    paths.truncate(PATHS_LISTED);

    let current_version = env!("CARGO_PKG_VERSION").to_owned();
    Ok(RestorePreviewResponse {
        archive_name: archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        archive_size,
        format_version: manifest.format_version,
        from_newer_version: newer(&manifest.app_version, &current_version),
        app_version: manifest.app_version,
        current_version,
        created_at: manifest.created_at,
        parts: groups
            .into_iter()
            .map(|(kind, (count, size))| RestorePartGroupResponse { kind, count, size })
            .collect(),
        storage_roots: bundle
            .as_ref()
            .map(|bundle| {
                bundle
                    .storage_roots
                    .iter()
                    .map(|root| RestoreRootResponse {
                        id: root.id.to_string(),
                        name: root.name.clone(),
                        native: native(&root.path),
                        path: root.path.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        paths,
        categories: bundle.as_ref().map_or(0, |bundle| bundle.categories.len()),
        accounts: bundle.as_ref().map_or(0, |bundle| bundle.accounts.len()),
        proxy_profiles: bundle
            .as_ref()
            .map_or(0, |bundle| bundle.proxy_profiles.len()),
        usenet_servers: bundle
            .as_ref()
            .map_or(0, |bundle| bundle.usenet_servers.len()),
        subscriptions: bundle
            .as_ref()
            .map_or(0, |bundle| bundle.subscriptions.len()),
        hotfolders: bundle.as_ref().map_or(0, |bundle| bundle.hotfolders.len()),
        credentials_included: bundle
            .as_ref()
            .is_some_and(|bundle| bundle.secrets.is_some()),
        plugin_trust_rows,
        partial_transfers: downloads.len(),
    })
}

/// Reads the whole archive with the passphrase and describes it; writes nothing.
#[utoipa::path(
    post,
    path = "/api/v1/backups/restore/preview",
    tag = "system",
    request_body = RestorePreviewRequest,
    responses(
        (status = 200, body = RestorePreviewResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn preview_restore(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<RestorePreviewRequest>,
) -> Result<Json<RestorePreviewResponse>, ApiError> {
    let _busy = Busy::take()?;
    Ok(Json(
        preview(&state, &audit, &request.source, &request.passphrase).await?,
    ))
}

/// Unpacks into a throwaway folder, migrates, remaps and checks the copy, then removes it.
#[utoipa::path(
    post,
    path = "/api/v1/backups/restore/test",
    tag = "system",
    request_body = RestoreRequest,
    responses(
        (status = 200, body = RestoreReportResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn test_restore(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<RestoreRequest>,
) -> Result<Json<RestoreReportResponse>, ApiError> {
    let _busy = Busy::take()?;
    let checked = restore_service::check(&state, &audit, &request, Mode::Test).await?;
    Ok(Json(checked.report))
}

fn status_of(state: &AppState) -> Result<RestoreStatusResponse, ApiError> {
    let layout = layout(state);
    if let Some(pending) = cutover::read_marker(&layout)? {
        return Ok(RestoreStatusResponse {
            state: match pending.phase {
                Phase::Staged => "staged",
                Phase::Switched => "switching",
            }
            .to_owned(),
            archive_name: Some(pending.archive_name),
            staged_at: Some(pending.staged_at),
            backup_created_at: Some(pending.backup_created_at),
            app_version: Some(pending.app_version),
            failed_at: None,
            reason: None,
        });
    }
    let failure = cutover::read_failure(&layout);
    Ok(RestoreStatusResponse {
        state: if failure.is_some() { "failed" } else { "none" }.to_owned(),
        archive_name: failure
            .as_ref()
            .and_then(|failure| failure.archive_name.clone()),
        staged_at: None,
        backup_created_at: None,
        app_version: None,
        failed_at: failure.as_ref().map(|failure| failure.failed_at),
        reason: failure.map(|failure| failure.reason),
    })
}

/// Checks the archive like a test restore and stages it; the next start switches to it.
/// Requires a signed-in session and the password.
#[utoipa::path(
    post,
    path = "/api/v1/backups/restore",
    tag = "system",
    request_body = RestoreRequest,
    responses(
        (status = 202, body = RestoreStagedResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
        (status = 403, description = "Not a signed-in session, or a wrong passphrase", body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn start_restore(
    State(state): State<AppState>,
    audit: AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
    client: crate::client::ClientAddress,
    Json(request): Json<RestoreRequest>,
) -> Result<(StatusCode, Json<RestoreStagedResponse>), ApiError> {
    rd_api_core::step_up::require_confirmed(
        &state,
        &audit,
        this_machine,
        client.0,
        request.password.as_deref(),
        AuditAction::BackupRestored,
    )
    .await?;
    let _busy = Busy::take()?;
    if cutover::read_marker(&layout(&state))?.is_some() {
        return Err(ApiError::conflict(
            "backup.restore_pending_exists",
            "A restore is already waiting for the next start",
        ));
    }
    let checked = restore_service::check(&state, &audit, &request, Mode::Restore).await?;
    if !checked.report.ok {
        let errors = checked
            .report
            .problems
            .iter()
            .filter(|problem| problem.severity == "error")
            .count();
        crate::restore_checks::forget_minted(&state, &checked.minted).await;
        if let Err(error) = tokio::fs::remove_dir_all(&checked.work).await {
            tracing::warn!(%error, "a refused restore's work folder could not be removed");
        }
        return Err(ApiError::unprocessable(
            "backup.restore_checks_failed",
            "The backup did not pass its checks; run a test restore to see why",
        )
        .with_param("count", errors));
    }
    let staged = restore_service::stage(&state, &audit, checked).await?;
    let report = staged.1;
    Ok((
        StatusCode::ACCEPTED,
        Json(RestoreStagedResponse {
            status: RestoreStatusResponse {
                state: "staged".to_owned(),
                archive_name: Some(staged.0.archive_name),
                staged_at: Some(staged.0.staged_at),
                backup_created_at: Some(staged.0.backup_created_at),
                app_version: Some(staged.0.app_version),
                failed_at: None,
                reason: None,
            },
            report,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/backups/restore",
    tag = "system",
    responses((status = 200, body = RestoreStatusResponse))
)]
pub async fn get_restore_status(
    State(state): State<AppState>,
) -> Result<Json<RestoreStatusResponse>, ApiError> {
    Ok(Json(status_of(&state)?))
}

/// Drops a restore that has not switched yet, with the credentials it put into the secret
/// store, or dismisses the record of one that did not start.
#[utoipa::path(
    delete,
    path = "/api/v1/backups/restore",
    tag = "system",
    responses((status = 200, body = RestoreStatusResponse), (status = 409, body = crate::error::ErrorBody))
)]
pub async fn discard_restore(
    State(state): State<AppState>,
    audit: AuditContext,
) -> Result<Json<RestoreStatusResponse>, ApiError> {
    let _busy = Busy::take()?;
    let discarded = cutover::discard(&layout(&state))
        .map_err(|error| ApiError::conflict("backup.restore_switching", format!("{error:#}")))?;
    if let Some(pending) = discarded {
        crate::restore_checks::forget_minted(&state, &pending.minted_secrets).await;
        crate::audit::record(
            &state,
            AuditEvent::success(AuditAction::BackupRestored)
                .by(&audit)
                .target("backup", "full_backup")
                .detail("step", "discarded")
                .detail("archive", &pending.archive_name),
        )
        .await;
    }
    Ok(Json(status_of(&state)?))
}

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::Hash;

use axum::{Json, extract::State};
use chrono::Utc;
use rd_api_core::input_checks::BundleHeader;

use crate::{
    ApiError, AppState,
    settings_backup_crypto::{decrypt_secrets, encrypt_secrets, encrypt_secrets_with_key},
};

pub use crate::settings_backup_dto::{
    BundleAccount, BundleIndexer, BundleProxyProfile, BundleStreamChannel, BundleUsenetServer,
    ExportSettingsRequest, ImportSettingsRequest, ImportSummaryResponse, SettingsBundle,
};

const BUNDLE_FORMAT: &str = "rdownloader-settings-bundle";
const BUNDLE_VERSION: u32 = 1;

mod checks;
mod export;

use checks::*;
pub use export::*;

#[utoipa::path(
    post,
    path = "/api/v1/settings/export",
    tag = "system",
    request_body = ExportSettingsRequest,
    responses((status = 200, body = SettingsBundle), (status = 400, body = crate::error::ErrorBody))
)]
pub async fn export_settings(
    State(state): State<AppState>,
    Json(request): Json<ExportSettingsRequest>,
) -> Result<Json<SettingsBundle>, ApiError> {
    let sealing = if request.include_secrets {
        let passphrase = request
            .passphrase
            .filter(|value| value.chars().count() >= 8);
        SecretSealing::Passphrase(passphrase.ok_or_else(passphrase_required)?)
    } else {
        SecretSealing::Omit
    };
    Ok(Json(build_settings_bundle(&state, sealing).await?))
}

/// Replaces all configuration tables atomically after validation. Secret-store writes made before
/// the database swap are removed on failure. A process crash after the swap but before settings
/// persistence can temporarily leave new tables with old settings; the next settings save heals it.
///
/// Requires a signed-in session and the password (RD-1190-19): the bundle replaces the accounts,
/// the hot folders and the settings the sign-in reads.
#[utoipa::path(
    post,
    path = "/api/v1/settings/import",
    tag = "system",
    request_body = ImportSettingsRequest,
    responses(
        (status = 200, body = ImportSummaryResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
        (status = 403, description = "Not a signed-in session", body = crate::error::ErrorBody)
    )
)]
pub async fn import_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
    client: crate::client::ClientAddress,
    Json(request): Json<ImportSettingsRequest>,
) -> Result<Json<ImportSummaryResponse>, ApiError> {
    rd_api_core::step_up::require_confirmed(
        &state,
        &audit,
        this_machine,
        client.0,
        request.password.as_deref(),
        rd_core::AuditAction::BackupRestored,
    )
    .await?;
    let mut bundle = request.bundle;
    validate_header(&bundle)?;
    validate_references(&bundle)?;
    crate::settings_handlers::validate_settings(&mut bundle.settings)?;
    // The bundle's roots replace the service's, so they pass the check a root created by hand
    // does — before anything is decrypted or written.
    let protected =
        crate::protected_roots::protected_directories(&state, Some(&bundle.settings)).await;
    for root in &bundle.storage_roots {
        crate::protected_roots::refuse_protected(std::path::Path::new(&root.path), &protected)?;
    }
    for path in bundle
        .hotfolders
        .iter()
        .filter(|hotfolder| matches!(hotfolder.executor, rd_core::HotFolderExecutor::Daemon))
        .flat_map(|hotfolder| {
            [
                &hotfolder.path,
                &hotfolder.processed_path,
                &hotfolder.failed_path,
            ]
        })
        .map(std::path::Path::new)
        .filter(|path| path.is_absolute())
    {
        crate::protected_roots::refuse_protected_hotfolder(path, &protected)?;
    }

    let secrets_included = bundle.secrets.is_some();
    let secret_values = match &bundle.secrets {
        Some(encrypted) => {
            let passphrase = request
                .passphrase
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(passphrase_required)?;
            decrypt_secrets(passphrase, encrypted).await?
        }
        None => BTreeMap::new(),
    };
    let needed_slots = referenced_slots(&bundle);
    crate::settings_backup_secrets::validate_secret_slots(&bundle, &secret_values, &needed_slots)?;
    let old_account_ids = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .map(|account| account.id)
        .collect::<Vec<_>>();
    let old_references = crate::settings_backup_secrets::current_secret_references(&state).await?;
    let minted = crate::settings_backup_secrets::mint_secret_references(
        &state,
        &secret_values,
        &needed_slots,
    )
    .await?;
    let minted_for_cleanup = minted.values().cloned().map(Some).collect::<Vec<_>>();
    let summary = ImportSummaryResponse::from_bundle(&bundle);
    let imported_account_ids = bundle
        .accounts
        .iter()
        .map(|account| account.id)
        .collect::<Vec<_>>();
    let settings = bundle.settings.clone();
    let replacement = into_replacement(bundle, &minted);
    if let Err(error) = state.database.replace_config(replacement).await {
        crate::config_fields::cleanup_secrets(&state.secrets, minted_for_cleanup).await;
        crate::audit::record(
            &state,
            crate::audit::AuditEvent::failure(rd_core::AuditAction::BackupRestored)
                .by(&audit)
                .target("backup", "configuration"),
        )
        .await;
        return Err(error.into());
    }
    crate::config_fields::cleanup_secrets(&state.secrets, old_references).await;
    for id in old_account_ids.into_iter().chain(imported_account_ids) {
        crate::hosters::forget(id);
    }
    crate::settings_handlers::apply_settings(&state, settings).await?;
    // The one action that replaces the whole configuration in a single request, and the one
    // the audit log most has to survive: `replace_config` clears configuration tables and
    // `audit_records` is not among them, so the record of a restore outlives the restore.
    // Counts only, never the contents: the bundle holds accounts, proxies and Usenet servers.
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::BackupRestored)
            .by(&audit)
            .target("backup", "configuration")
            .detail("storage_roots", summary.storage_roots)
            .detail("categories", summary.categories)
            .detail("accounts", summary.accounts)
            .detail("usenet_servers", summary.usenet_servers)
            .detail("with_secrets", secrets_included),
    )
    .await;
    Ok(Json(summary))
}

/// What a settings bundle says about itself; only the current version is read.
const BUNDLE_HEADER: BundleHeader = BundleHeader {
    format: BUNDLE_FORMAT,
    version: BUNDLE_VERSION,
    reads_older: false,
    format_code: "settings.backup_invalid",
    format_message: "The selected file is not an rDownloader settings bundle",
    version_code: "settings.backup_version_unsupported",
    version_message: "This settings bundle version is not supported",
};

pub(crate) fn validate_header(bundle: &SettingsBundle) -> Result<(), ApiError> {
    BUNDLE_HEADER.check(&bundle.format, bundle.version)
}

fn passphrase_required() -> ApiError {
    ApiError::bad_request(
        "settings.backup_passphrase_required",
        "A passphrase of at least 8 characters is required for a backup with secrets",
    )
}

pub(crate) fn invalid_bundle(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("settings.backup_invalid", message)
}

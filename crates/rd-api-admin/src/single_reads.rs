//! `GET <collection>/{id}` of the configuration (RD-1480-10, API-6): one row as its list shows it.
//!
//! Every handler keeps one row of its list, so the two cannot drift apart in shape or in what
//! they reveal (`rd_api_core::single_read`). The route costs what the list costs.

use axum::extract::State;
use rd_api_core::extract::{Json, Path};
use rd_api_core::single_read::one_of;

use crate::{ApiError, AppState, error_codes::parse_id};

/// One provider account, without its credentials.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/accounts/{id}", tag = "configuration", params(("id" = rd_core::AccountId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::Account), (status = 404)))]
pub async fn get_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::Account>, ApiError> {
    let id = parse_id::<rd_core::AccountId>(&id)?;
    let Json(rows) = crate::config_handlers::list_accounts(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        crate::error_codes::account_not_found,
    )
}

/// One automation with the definition in force.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/automations/{id}", tag = "automations", params(("id" = rd_core::AutomationId, Path, description = "The id the list shows")), responses((status = 200, body = crate::automation_handlers::AutomationResponse), (status = 404)))]
pub async fn get_automation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::automation_handlers::AutomationResponse>, ApiError> {
    let id = parse_id::<rd_core::AutomationId>(&id)?;
    let Json(rows) = crate::automation_handlers::list_automations(State(state)).await?;
    one_of(
        rows,
        |row| row.automation.id == id,
        || ApiError::not_found("automation.not_found", "Automation not found"),
    )
}

/// One destination of the full backup, as `GET /api/v1/backups` lists it.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/backups/destinations/{id}", tag = "system", params(("id" = String, Path, description = "The id the list shows")), responses((status = 200, body = crate::backup_destination_handlers::BackupDestinationResponse), (status = 404)))]
pub async fn get_backup_destination(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::backup_destination_handlers::BackupDestinationResponse>, ApiError> {
    let Json(rows) = crate::backup_handlers::get_backup_config(State(state)).await?;
    one_of(
        rows.destinations,
        |row| row.id == id,
        || {
            ApiError::not_found(
                "backup.destination_not_found",
                "No backup destination has this id",
            )
        },
    )
}

/// One category.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/categories/{id}", tag = "configuration", params(("id" = rd_core::CategoryId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::Category), (status = 404)))]
pub async fn get_category(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::Category>, ApiError> {
    let id = parse_id::<rd_core::CategoryId>(&id)?;
    let Json(rows) = crate::config_handlers::list_categories(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("category.not_found", "Category not found"),
    )
}

/// One routing rule.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/category-rules/{id}", tag = "configuration", params(("id" = rd_core::CategoryRuleId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::CategoryRule), (status = 404)))]
pub async fn get_category_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::CategoryRule>, ApiError> {
    let id = parse_id::<rd_core::CategoryRuleId>(&id)?;
    let Json(rows) = crate::config_handlers::list_category_rules(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("category_rule.not_found", "Category rule not found"),
    )
}

/// One watched folder.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/hotfolders/{id}", tag = "configuration", params(("id" = rd_core::HotFolderId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::HotFolderConfig), (status = 404)))]
pub async fn get_hotfolder(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::HotFolderConfig>, ApiError> {
    let id = parse_id::<rd_core::HotFolderId>(&id)?;
    let Json(rows) = crate::config_handlers::list_hotfolders(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("hotfolder.not_found", "Hotfolder not found"),
    )
}

/// One notification rule.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/notifications/rules/{id}", tag = "notifications", params(("id" = rd_core::NotificationRuleId, Path, description = "The id the list shows")), responses((status = 200, body = rd_notify::NotificationRule), (status = 404)))]
pub async fn get_notification_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_notify::NotificationRule>, ApiError> {
    let id = parse_id::<rd_core::NotificationRuleId>(&id)?;
    let Json(rows) = crate::notify_handlers::list_rules(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("notification.rule_not_found", "Notification rule not found"),
    )
}

/// One notification target, without its secret.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/notifications/targets/{id}", tag = "notifications", params(("id" = rd_core::NotificationTargetId, Path, description = "The id the list shows")), responses((status = 200, body = rd_notify::NotificationTarget), (status = 404)))]
pub async fn get_notification_target(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_notify::NotificationTarget>, ApiError> {
    let id = parse_id::<rd_core::NotificationTargetId>(&id)?;
    let Json(rows) = crate::notify_handlers::list_targets(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || {
            ApiError::not_found(
                "notification.target_not_found",
                "Notification target not found",
            )
        },
    )
}

/// One object storage profile, without its keys.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/object-storage/profiles/{id}", tag = "configuration", params(("id" = rd_core::ObjectStorageProfileId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::ObjectStorageProfile), (status = 404)))]
pub async fn get_object_storage_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::ObjectStorageProfile>, ApiError> {
    let id = parse_id::<rd_core::ObjectStorageProfileId>(&id)?;
    let Json(rows) =
        crate::object_storage_handlers::list_object_storage_profiles(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || {
            ApiError::not_found(
                "object_storage.profile_not_found",
                "Object storage profile not found",
            )
        },
    )
}

/// One plugin repository.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/plugins/repositories/{id}", tag = "plugins", params(("id" = String, Path, description = "The id the list shows")), responses((status = 200, body = crate::plugin_repository_dto::PluginRepositoryResponse), (status = 404)))]
pub async fn get_plugin_repository(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::plugin_repository_dto::PluginRepositoryResponse>, ApiError> {
    let Json(rows) =
        crate::plugin_repository_handlers::list_plugin_repositories(State(state)).await?;
    one_of(
        rows.repositories,
        |row| row.id == id,
        || {
            ApiError::not_found(
                "plugin_repository.not_found",
                "The plugin repository does not exist",
            )
        },
    )
}

/// One proxy profile, without its password.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/proxy-profiles/{id}", tag = "configuration", params(("id" = rd_core::ProxyProfileId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::ProxyProfile), (status = 404)))]
pub async fn get_proxy_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::ProxyProfile>, ApiError> {
    let id = parse_id::<rd_core::ProxyProfileId>(&id)?;
    let Json(rows) = crate::config_handlers::list_proxy_profiles(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("proxy.not_found", "Proxy profile not found"),
    )
}

/// One stored FTP or SFTP login, without its password and key.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/remote-credentials/{id}", tag = "configuration", params(("id" = rd_core::RemoteCredentialId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::RemoteCredential), (status = 404)))]
pub async fn get_remote_credential(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::RemoteCredential>, ApiError> {
    let id = parse_id::<rd_core::RemoteCredentialId>(&id)?;
    let Json(rows) = crate::remote_handlers::list_remote_credentials(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("remote.credential_not_found", "Remote login not found"),
    )
}

/// One storage root.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/storage-roots/{id}", tag = "configuration", params(("id" = rd_core::StorageRootId, Path, description = "The id the list shows")), responses((status = 200, body = crate::dto::StorageRootResponse), (status = 404)))]
pub async fn get_storage_root(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::dto::StorageRootResponse>, ApiError> {
    let id = parse_id::<rd_core::StorageRootId>(&id)?;
    let Json(rows) = crate::config_handlers::list_storage_roots(State(state)).await?;
    one_of(
        rows,
        |row| row.root.id == id,
        || ApiError::not_found("storage_root.not_found", "Storage root not found"),
    )
}

/// Everything the inventory holds about one plugin: its installed versions, the ones this build
/// cannot load, and its lifecycle.
///
/// The inventory's own shape rather than one row, because a plugin id has several rows: each
/// installed version is one.
#[utoipa::path(get, path = "/api/v1/plugins/{id}", tag = "plugins", params(("id" = String, Path, description = "The id the list shows")), responses((status = 200, body = crate::plugin_lifecycle::PluginInventoryResponse), (status = 404)))]
pub async fn get_plugin(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::plugin_lifecycle::PluginInventoryResponse>, ApiError> {
    let Json(mut inventory) = crate::plugin_handlers::list_plugins(State(state)).await?;
    inventory.installed.retain(|row| row.id.to_string() == id);
    inventory.incompatible.retain(|row| row.id == id);
    inventory.lifecycle.retain(|row| row.plugin_id == id);
    if inventory.installed.is_empty() && inventory.incompatible.is_empty() {
        return Err(ApiError::not_found(
            "plugin.not_installed",
            "This plugin is not installed",
        ));
    }
    Ok(Json(inventory))
}

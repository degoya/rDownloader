//! The service settings document: read, validated, saved, reset and applied live.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderName, header},
};
use rd_api_core::extract::Json;
use rd_api_core::settings_change::{announce, settings_etag};

use crate::{
    ApiError, AppState,
    dto::SettingsResponse,
    error::ErrorBody,
    settings_store::{read_settings, runtime_settings},
};

mod edit;
mod privileged;
mod validation;

pub use edit::SettingsEdit;
use privileged::privileged_change;
pub(crate) use validation::validate_settings;

/// The settings document with its entity tag (RD-1480-07).
type TaggedSettings = ([(HeaderName, String); 1], Json<SettingsResponse>);

fn tagged(settings: SettingsResponse) -> Result<TaggedSettings, ApiError> {
    let etag = settings_etag(&settings)?;
    Ok(([(header::ETAG, etag)], Json(settings)))
}

#[utoipa::path(get, path = "/api/v1/settings", tag = "system", responses((status = 200, body = SettingsResponse, headers(("etag" = String, description = "The tag of this document; `If-Match` on a write names it")))))]
pub async fn get_settings(State(state): State<AppState>) -> Result<TaggedSettings, ApiError> {
    tagged(read_settings(&state).await?)
}

/// Replaces the settings document.
///
/// A field left out of the body falls back to its default; `PATCH` changes single fields.
#[utoipa::path(put, path = "/api/v1/settings", tag = "system", params(("If-Match" = Option<String>, Header, description = "The `ETag` the document was read with; another one is answered `412`")), request_body = SettingsResponse, responses((status = 200, body = SettingsResponse, headers(("etag" = String, description = "The tag of the document as saved"))), (status = 409, description = "Saved, but the torrent engine kept its previous settings (`torrent.session_rebuild_failed`; `torrent.port_in_use` when another program holds the fixed listen port)"), (status = 412, body = ErrorBody, description = "`settings.document_changed`: the stored document is not the one `If-Match` names; nothing was saved")))]
pub async fn put_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    headers: HeaderMap,
    Json(settings): Json<SettingsResponse>,
) -> Result<TaggedSettings, ApiError> {
    let edit = SettingsEdit::begin(&state).await?;
    edit.require_match(&headers)?;
    tagged(
        edit.save_settings(&audit, holds_admin(granted), settings)
            .await?,
    )
}

/// Changes the fields the body names and leaves every other one as stored.
///
/// A field named is replaced whole, an object or a list in it included; `null` empties a field
/// that can be empty and is refused for any other.
#[utoipa::path(patch, path = "/api/v1/settings", tag = "system", params(("If-Match" = Option<String>, Header, description = "The `ETag` the document was read with; another one is answered `412`")), request_body = SettingsResponse, responses((status = 200, body = SettingsResponse, headers(("etag" = String, description = "The tag of the document as saved"))), (status = 400, body = ErrorBody, description = "`settings.patch_empty`, `settings.unknown_key` or `settings.invalid`, the last two with the field in `key`, or the code of a value that is out of its range"), (status = 403, body = ErrorBody, description = "`auth.scope_insufficient`: the field in `setting` takes `api:admin`"), (status = 409, body = ErrorBody, description = "Saved, but the torrent engine kept its previous settings (`torrent.session_rebuild_failed`, `torrent.port_in_use`)"), (status = 412, body = ErrorBody, description = "`settings.document_changed`: the stored document is not the one `If-Match` names; nothing was saved")))]
pub async fn patch_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    headers: HeaderMap,
    Json(patch): Json<serde_json::Map<String, serde_json::Value>>,
) -> Result<TaggedSettings, ApiError> {
    let edit = SettingsEdit::begin(&state).await?;
    edit.require_match(&headers)?;
    let settings = edit.patched(patch)?;
    tagged(
        edit.save_settings(&audit, holds_admin(granted), settings)
            .await?,
    )
}

fn holds_admin(granted: Option<axum::Extension<crate::auth::Granted>>) -> bool {
    granted.is_some_and(|axum::Extension(granted)| granted.holds(rd_core::Scope::Admin))
}

/// Restores the built-in runtime defaults without touching accounts, routing or download data.
#[utoipa::path(post, path = "/api/v1/settings/reset", tag = "system", responses((status = 200, body = SettingsResponse)))]
pub async fn reset_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
) -> Result<TaggedSettings, ApiError> {
    let applied = apply_settings(&state, SettingsResponse::default()).await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsReset)
            .by(&audit)
            .target("settings", "service.settings"),
    )
    .await;
    tagged(applied)
}

/// Validates, persists and live-applies the full settings blob, in its own turn.
///
/// A torrent session that could not be rebuilt for the new settings is logged and keeps its
/// previous ones; [`SettingsEdit::save_settings`] hands that refusal to the caller instead.
pub(crate) async fn apply_settings(
    state: &AppState,
    settings: SettingsResponse,
) -> Result<SettingsResponse, ApiError> {
    let _turn = state.admin.settings_turn.take().await;
    // A stored document that does not parse is what a reset or an import repairs.
    let before = read_settings(state).await.ok();
    Ok(apply_holding_turn(state, before.as_ref(), settings)
        .await?
        .0)
}

/// [`apply_settings`] for a caller that holds the turn and knows the document before, answering
/// beside the stored document the torrent engine's refusal of it, if it refused: the document is
/// saved and every other part applied either way.
async fn apply_holding_turn(
    state: &AppState,
    before: Option<&SettingsResponse>,
    mut settings: SettingsResponse,
) -> Result<(SettingsResponse, Option<ApiError>), ApiError> {
    let runtime = validate_settings(&mut settings)?;
    state
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    announce(&state.database, before, &settings);
    state.scheduler.update_runtime_settings(runtime).await?;
    // The level holds from the next line (RD-1260-12); a debug window still open stays open.
    crate::log_level_handlers::apply_level(settings.log_level);
    // Every running watcher re-arms its ticker on this; nothing is restarted (RD-110-31).
    state
        .intake
        .hotfolders
        .set_poll_interval(crate::hotfolder_service::poll_interval_of(&settings));
    // The thresholds live in the same blob, so the capacity service reloads right away
    // instead of waiting for the next supervision tick.
    state.scheduler.reload_capacity_config().await?;
    // The schedule's timezone and default profile ride in the same blob.
    state.scheduler.reload_bandwidth().await?;
    let timezone = rd_limits::parse_timezone(&settings.bandwidth_timezone)
        .unwrap_or(rd_limits::default_timezone());
    state
        .queue
        .power
        .apply(
            rd_power::PowerSettings {
                quiet_hours: settings.quiet_hours.clone(),
                quiet_hours_defer_postprocess: settings.quiet_hours_defer_postprocess,
                quiet_hours_defer_notifications: settings.quiet_hours_defer_notifications,
                completion_action: settings.completion_action,
                completion_script: settings.completion_script.clone(),
                completion_countdown_seconds: settings.completion_countdown_seconds,
                power_actions_allowed: settings.power_actions_allowed,
                pause_on_battery: settings.pause_on_battery,
                pause_on_metered: settings.pause_on_metered,
                prevent_standby: settings.prevent_standby,
                prevent_display_standby: settings.prevent_display_standby,
            },
            timezone,
        )
        .await;
    state
        .access
        .auth
        .set_disabled(settings.admin_login_disabled);
    // Every following request is measured against these, the sessions already open included.
    state
        .access
        .auth
        .set_session_limits(rd_core::SessionLimits {
            idle_hours: settings.session_idle_hours,
            max_hours: settings.session_max_hours,
        });
    // Validated above, so this cannot fail; falling back to the safe default rather than
    // unwrapping keeps a future refactor of the validator from turning into a panic.
    *state.access.proxy.write().await =
        crate::host_check::proxy_config(&settings).unwrap_or_default();
    *state.intake.media_settings.write().await = rd_core::MediaSettings {
        media_ytdlp_executable: settings.media_ytdlp_executable.clone(),
        media_ffmpeg_executable: settings.media_ffmpeg_executable.clone(),
        media_default_variant: settings.media_default_variant.clone(),
        media_default_criteria: settings.media_default_criteria.clone(),
        media_output_template: settings.media_output_template.clone(),
        media_hosts: settings.media_hosts.clone(),
        media_max_parallel: settings.media_max_parallel,
        media_check_timeout_seconds: settings.media_check_timeout_seconds,
        media_sleep_requests_seconds: settings.media_sleep_requests_seconds,
        media_sleep_interval_seconds: settings.media_sleep_interval_seconds,
        vendor_directory: settings.vendor_directory.clone(),
    };
    *state.intake.gallery_settings.write().await = rd_core::GallerySettings {
        gallery_executable: settings.gallery_executable.clone(),
        gallery_hosts: settings.gallery_hosts.clone(),
        gallery_max_parallel: settings.gallery_max_parallel,
        vendor_directory: settings.vendor_directory.clone(),
    };
    state
        .admin
        .tools
        .apply_settings(rd_core::ManagedToolSettings {
            managed_tools_enabled: settings.managed_tools_enabled,
            managed_tools_manifest_url: settings.managed_tools_manifest_url.clone(),
            tool_compatibility_overrides: settings.tool_compatibility_overrides.clone(),
        });
    *state.queue.remote_settings.write().await = rd_core::RemoteSettings {
        remote_max_parallel: settings.remote_max_parallel,
        remote_timeout_seconds: settings.remote_timeout_seconds,
        remote_ssh_auto_trust: settings.remote_ssh_auto_trust,
    }
    .sanitized();
    *state.intake.stream_settings.write().await = rd_core::StreamSettings {
        record_streamlink_executable: settings.record_streamlink_executable.clone(),
        record_default_quality: settings.record_default_quality.clone(),
        record_poll_interval_seconds: settings.record_poll_interval_seconds,
        record_max_parallel: settings.record_max_parallel,
        vendor_directory: settings.vendor_directory.clone(),
    };
    // Ratio/time/seeding apply live; port and rate limits on the next session start.
    *state.queue.torrent_settings.write().await = rd_core::TorrentSettings {
        torrent_listen_port: settings.torrent_listen_port,
        torrent_seed_ratio: settings.torrent_seed_ratio,
        torrent_seed_time_minutes: settings.torrent_seed_time_minutes,
        torrent_seeding_enabled: settings.torrent_seeding_enabled,
        torrent_sharing_enabled: settings.torrent_sharing_enabled,
        torrent_upload_limit_bytes_per_second: settings.torrent_upload_limit_bytes_per_second,
        torrent_bind_interface: settings.torrent_bind_interface.clone(),
        torrent_kill_switch_enabled: settings.torrent_kill_switch_enabled,
        torrent_ip_blocklist_url: settings.torrent_ip_blocklist_url.clone(),
        torrent_listen_mode: settings.torrent_listen_mode,
        torrent_peer_limit: settings.torrent_peer_limit,
        torrent_download_limit_bytes_per_second: settings.torrent_download_limit_bytes_per_second,
        torrent_proxy_profile_id: settings.torrent_proxy_profile_id,
        torrent_upnp_enabled: settings.torrent_upnp_enabled,
        torrent_announce_port: settings.torrent_announce_port,
        torrent_peer_addresses_visible: settings.torrent_peer_addresses_visible,
        torrent_max_active_downloads: settings.torrent_max_active_downloads,
        torrent_max_active_seeds: settings.torrent_max_active_seeds,
        keep_import_history: settings.keep_import_history,
    };
    // Rate limits are applied in place; a changed listen port rebuilds the session. A
    // failed rebuild keeps the previous engine running, so it is reported, not fatal.
    let torrent_refused = match state.queue.torrent.reconfigure().await {
        Ok(()) => None,
        Err(error) => {
            let reason = format!("{error:#}");
            tracing::warn!(error = %reason, "torrent session could not be reconfigured");
            Some(
                match rd_api_core::torrent_intake::torrent_port_in_use(&error) {
                    Some(port) => torrent_port_in_use(port, &reason),
                    None => torrent_rebuild_failed(&reason),
                },
            )
        }
    };
    // A lowered seed limit ends the surplus seeds now rather than at the next tick.
    state.queue.torrent.nudge_seeding();
    Ok((settings, torrent_refused))
}

/// `409` for settings that were saved but the torrent engine could not take.
fn torrent_rebuild_failed(reason: &str) -> ApiError {
    // The cause can name a proxy address; it reaches a browser and an MCP client.
    let reason = rd_core::redact_text(reason);
    ApiError::conflict(
        "torrent.session_rebuild_failed",
        format!(
            "The settings were saved, but the torrent engine could not take them and keeps its \
             previous ones: {reason}"
        ),
    )
    .with_param("reason", reason)
}

/// `409` for a saved fixed listen port another program holds (RD-1250-04): the engine keeps its
/// previous settings, as for any refused rebuild, and the answer names the port.
fn torrent_port_in_use(port: u16, reason: &str) -> ApiError {
    ApiError::conflict(
        "torrent.port_in_use",
        format!(
            "The settings were saved, but another program holds port {port}; the torrent engine \
             keeps its previous settings: {}",
            rd_core::redact_text(reason)
        ),
    )
    .with_param("port", port)
}

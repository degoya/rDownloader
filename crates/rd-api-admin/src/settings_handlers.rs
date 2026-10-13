//! The service settings document: read, validated, saved, reset and applied live.

use axum::{Json, extract::State};

use crate::{
    ApiError, AppState,
    dto::SettingsResponse,
    settings_store::{read_settings, runtime_settings},
};

mod privileged;
mod validation;

use privileged::privileged_change;
pub(crate) use validation::validate_settings;

#[utoipa::path(get, path = "/api/v1/settings", tag = "system", responses((status = 200, body = SettingsResponse)))]
pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<SettingsResponse>, ApiError> {
    Ok(Json(read_settings(&state).await?))
}

#[utoipa::path(put, path = "/api/v1/settings", tag = "system", request_body = SettingsResponse, responses((status = 200, body = SettingsResponse), (status = 409, description = "Saved, but the torrent engine kept its previous settings (`torrent.session_rebuild_failed`; `torrent.port_in_use` when another program holds the fixed listen port)")))]
pub async fn put_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Json(settings): Json<SettingsResponse>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let holds_admin =
        granted.is_some_and(|axum::Extension(granted)| granted.holds(rd_core::Scope::Admin));
    save_settings(&state, &audit, holds_admin, settings)
        .await
        .map(Json)
}

/// Saves a settings document on a caller's behalf: the privileged-field gate, the apply and
/// the audit record, in the one function both the REST route and the MCP tool call.
///
/// The gate used to live in [`put_settings`] alone, and the MCP tool `update_settings` went
/// straight to [`apply_settings`]. Both cost `api:config`, so a configuration token could
/// switch the administrator login off — and with it gain every scope — over MCP while the
/// same change over REST was refused (RD-130-09). One function is what keeps the two from
/// drifting apart again. `holds_admin` is the caller's own grant, never the service's.
pub async fn save_settings(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    holds_admin: bool,
    settings: SettingsResponse,
) -> Result<SettingsResponse, ApiError> {
    let current = read_settings(state).await?;
    if !holds_admin && let Some(field) = privileged_change(&current, &settings) {
        // Audited as a failure: a refused attempt to widen what a credential may reach is
        // exactly the thing somebody reads an audit log to find.
        crate::audit::record(
            state,
            crate::audit::AuditEvent::failure(rd_core::AuditAction::SettingsChanged)
                .by(audit)
                .target("settings", "service.settings")
                .detail("refused_field", field)
                .detail("reason", "scope_insufficient"),
        )
        .await;
        return Err(ApiError::forbidden(
            "auth.scope_insufficient",
            "This setting requires the administration scope",
        )
        .with_param("scope", rd_core::Scope::Admin.as_str())
        .with_param("setting", field));
    }
    // Only a changed target: a stored one whose profile went since must not hold up every
    // other setting on the page (RD-1190-20).
    if settings.upload_remote != current.upload_remote {
        rd_api_core::object_upload_target::check_object_upload_target(
            &state.database,
            settings.upload_remote.as_deref(),
        )
        .await?;
    }
    let (applied, torrent_refused) = apply_settings_reporting(state, settings).await?;
    // The *names* of the fields that changed, never their values: the settings document holds
    // secret references, executable paths and proxy addresses, and an audit log that quoted
    // them would be a copy of the configuration with a timestamp on it.
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(audit)
            .target("settings", "service.settings")
            .detail("fields", changed_field_names(&current, &applied).join(" ")),
    )
    .await;
    // Saved and audited, but the torrent engine kept its previous settings: said, not a
    // silent `200` beside a status that shows the new values (RD-1240-28).
    if let Some(refused) = torrent_refused {
        return Err(refused);
    }
    Ok(applied)
}

/// The names of the settings fields whose stored value differs between two documents.
///
/// Compared as JSON rather than field by field so a field added later is covered without
/// anybody remembering to add it here, and so nothing in this function ever has to hold a
/// value long enough to log it by accident.
fn changed_field_names(current: &SettingsResponse, next: &SettingsResponse) -> Vec<String> {
    let (Ok(serde_json::Value::Object(before)), Ok(serde_json::Value::Object(after))) =
        (serde_json::to_value(current), serde_json::to_value(next))
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect();
    names.extend(
        before
            .keys()
            .filter(|key| !after.contains_key(*key))
            .cloned(),
    );
    names.sort();
    names
}

/// Restores the built-in runtime defaults without touching accounts, routing or download data.
#[utoipa::path(post, path = "/api/v1/settings/reset", tag = "system", responses((status = 200, body = SettingsResponse)))]
pub async fn reset_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
) -> Result<Json<SettingsResponse>, ApiError> {
    let applied = apply_settings(&state, SettingsResponse::default()).await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsReset)
            .by(&audit)
            .target("settings", "service.settings"),
    )
    .await;
    Ok(Json(applied))
}

/// Validates, persists and live-applies the full settings blob — the only legal mutation path.
///
/// A torrent session that could not be rebuilt for the new settings is logged and keeps its
/// previous ones; [`apply_settings_reporting`] hands that refusal to the caller instead.
pub(crate) async fn apply_settings(
    state: &AppState,
    settings: SettingsResponse,
) -> Result<SettingsResponse, ApiError> {
    Ok(apply_settings_reporting(state, settings).await?.0)
}

/// [`apply_settings`], answering beside the stored document the torrent engine's refusal of
/// it, if it refused: the document is saved and every other part applied either way.
pub(crate) async fn apply_settings_reporting(
    state: &AppState,
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
    state.scheduler.update_runtime_settings(runtime).await?;
    // The level holds from the next line (RD-1260-12); a debug window still open stays open.
    crate::log_level_handlers::apply_level(settings.log_level);
    // Every running watcher re-arms its ticker on this; nothing is restarted (RD-110-31).
    state
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
    state.auth.set_disabled(settings.admin_login_disabled);
    // Every following request is measured against these, the sessions already open included.
    state.auth.set_session_limits(rd_core::SessionLimits {
        idle_hours: settings.session_idle_hours,
        max_hours: settings.session_max_hours,
    });
    // Validated above, so this cannot fail; falling back to the safe default rather than
    // unwrapping keeps a future refactor of the validator from turning into a panic.
    *state.proxy.write().await = crate::host_check::proxy_config(&settings).unwrap_or_default();
    *state.media_settings.write().await = rd_core::MediaSettings {
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
    *state.gallery_settings.write().await = rd_core::GallerySettings {
        gallery_executable: settings.gallery_executable.clone(),
        gallery_hosts: settings.gallery_hosts.clone(),
        gallery_max_parallel: settings.gallery_max_parallel,
        vendor_directory: settings.vendor_directory.clone(),
    };
    state.tools.apply_settings(rd_core::ManagedToolSettings {
        managed_tools_enabled: settings.managed_tools_enabled,
        managed_tools_manifest_url: settings.managed_tools_manifest_url.clone(),
        tool_compatibility_overrides: settings.tool_compatibility_overrides.clone(),
    });
    *state.remote_settings.write().await = rd_core::RemoteSettings {
        remote_max_parallel: settings.remote_max_parallel,
        remote_timeout_seconds: settings.remote_timeout_seconds,
        remote_ssh_auto_trust: settings.remote_ssh_auto_trust,
    }
    .sanitized();
    *state.stream_settings.write().await = rd_core::StreamSettings {
        record_streamlink_executable: settings.record_streamlink_executable.clone(),
        record_default_quality: settings.record_default_quality.clone(),
        record_poll_interval_seconds: settings.record_poll_interval_seconds,
        record_max_parallel: settings.record_max_parallel,
        vendor_directory: settings.vendor_directory.clone(),
    };
    // Ratio/time/seeding apply live; port and rate limits on the next session start.
    *state.torrent_settings.write().await = rd_core::TorrentSettings {
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
    let torrent_refused = match state.torrent.reconfigure().await {
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
    state.torrent.nudge_seeding();
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

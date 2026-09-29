//! The service settings document: read, validated, saved, reset and applied live.

use axum::{Json, extract::State};

use crate::{
    ApiError, AppState,
    dto::SettingsResponse,
    settings_store::{read_settings, service_switches},
};

#[utoipa::path(get, path = "/api/v1/settings", tag = "system", responses((status = 200, body = SettingsResponse)))]
pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<SettingsResponse>, ApiError> {
    Ok(Json(read_settings(&state).await?))
}

#[utoipa::path(put, path = "/api/v1/settings", tag = "system", request_body = SettingsResponse, responses((status = 200, body = SettingsResponse)))]
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
    let applied = apply_settings(state, settings).await?;
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

/// The first privileged field `next` changes, if any.
///
/// The settings blob is priced `api:config`, but a handful of its fields do not configure
/// downloading at all — they decide who may talk to the service and what it executes.
/// `admin_login_disabled` is the sharpest: setting it makes `granted_scopes` hand every
/// caller, authenticated or not, the full `Scope::API`, so an `api:config` token could mint
/// itself `api:secrets` and `api:admin`. The executable paths and the completion script are
/// the same problem one step removed: they name a program this service runs.
fn privileged_change(current: &SettingsResponse, next: &SettingsResponse) -> Option<&'static str> {
    let fields: [(&'static str, bool); 16] = [
        (
            "admin_login_disabled",
            current.admin_login_disabled != next.admin_login_disabled,
        ),
        (
            "trusted_proxies",
            current.trusted_proxies != next.trusted_proxies,
        ),
        ("external_url", current.external_url != next.external_url),
        // Which names a browser may reach the service by: the rebinding guard (security
        // review 2026-09-28, finding 3), so as privileged as the proxy contract it sits in.
        ("allowed_hosts", current.allowed_hosts != next.allowed_hosts),
        (
            "cookie_security",
            current.cookie_security != next.cookie_security,
        ),
        // How long a stolen cookie stays worth something: lengthening either limit widens
        // what a credential reaches in time the way the fields above widen it in scope
        // (RD-130-09).
        (
            "session_idle_hours",
            current.session_idle_hours != next.session_idle_hours,
        ),
        (
            "session_max_hours",
            current.session_max_hours != next.session_max_hours,
        ),
        (
            "completion_action",
            current.completion_action != next.completion_action,
        ),
        (
            "completion_script",
            current.completion_script != next.completion_script,
        ),
        (
            "scripts_directory",
            current.scripts_directory != next.scripts_directory,
        ),
        ("custom_ca_pem", current.custom_ca_pem != next.custom_ca_pem),
        (
            "vendor_directory",
            current.vendor_directory != next.vendor_directory,
        ),
        (
            "rar_executable",
            current.rar_executable != next.rar_executable,
        ),
        (
            "remote_ssh_auto_trust",
            current.remote_ssh_auto_trust != next.remote_ssh_auto_trust,
        ),
        // Pointing the trace export somewhere is telling the service to post to a URL of
        // somebody's choosing on a timer. That is the same family of decision as naming a
        // program it runs, whatever the payload is (RD-110-03).
        (
            "otlp_endpoint",
            current.otlp_endpoint != next.otlp_endpoint
                || current.otlp_enabled != next.otlp_enabled,
        ),
        (
            "media_ytdlp_executable",
            current.media_ytdlp_executable != next.media_ytdlp_executable
                || current.media_ffmpeg_executable != next.media_ffmpeg_executable
                || current.gallery_executable != next.gallery_executable
                || current.record_streamlink_executable != next.record_streamlink_executable,
        ),
    ];
    fields
        .into_iter()
        .find_map(|(name, changed)| changed.then_some(name))
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
pub(crate) async fn apply_settings(
    state: &AppState,
    mut settings: SettingsResponse,
) -> Result<SettingsResponse, ApiError> {
    let runtime = validate_settings(&mut settings)?;
    state
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    state.scheduler.update_runtime_settings(runtime).await?;
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
        keep_import_history: settings.keep_import_history,
    };
    // Rate limits are applied in place; a changed listen port rebuilds the session. A
    // failed rebuild keeps the previous engine running, so it is reported, not fatal.
    if let Err(error) = state.torrent.reconfigure().await {
        tracing::warn!(%error, "torrent session could not be reconfigured");
    }
    Ok(settings)
}

pub(crate) fn validate_settings(
    settings: &mut SettingsResponse,
) -> Result<rd_scheduler::RuntimeSettings, ApiError> {
    // Parsed here rather than at the point of use so a bad range or URL is refused when it is
    // saved, with a message naming the value, instead of quietly disabling proxy trust later.
    crate::host_check::proxy_config(settings).map_err(|error| {
        ApiError::bad_request(
            "settings.proxy_invalid",
            "The proxy configuration is not usable",
        )
        .with_param("reason", error.to_string())
    })?;
    // Refused here rather than clamped: a session limit is a security setting, and quietly
    // storing a different value than the one somebody typed would be a decision they did
    // not take (RD-130-09).
    let (idle_range, max_range) = (
        rd_core::SESSION_IDLE_HOURS_RANGE,
        rd_core::SESSION_MAX_HOURS_RANGE,
    );
    if !idle_range.contains(&settings.session_idle_hours) {
        return Err(ApiError::bad_request(
            "settings.session_idle_invalid",
            format!(
                "The idle limit of a sign-in must be between {} and {} hours",
                idle_range.start(),
                idle_range.end()
            ),
        )
        .with_param("min", *idle_range.start())
        .with_param("max", *idle_range.end()));
    }
    if !max_range.contains(&settings.session_max_hours) {
        return Err(ApiError::bad_request(
            "settings.session_max_invalid",
            format!(
                "The maximum lifetime of a sign-in must be between {} and {} hours",
                max_range.start(),
                max_range.end()
            ),
        )
        .with_param("min", *max_range.start())
        .with_param("max", *max_range.end()));
    }
    if !matches!(settings.byte_display.as_str(), "binary" | "decimal") {
        return Err(ApiError::bad_request(
            "settings.byte_display_invalid",
            "Byte display must be binary or decimal",
        ));
    }
    if !matches!(
        settings.byte_unit.as_str(),
        "auto" | "byte" | "kilo" | "mega" | "giga" | "tera" | "peta"
    ) {
        return Err(ApiError::bad_request(
            "settings.byte_unit_invalid",
            "The byte unit must be auto, byte, kilo, mega, giga, tera or peta",
        ));
    }
    if !rd_core::LOG_RETENTION_RECORDS_RANGE.contains(&settings.log_retention_records)
        || !rd_core::LOG_RETENTION_DAYS_RANGE.contains(&settings.log_retention_days)
    {
        return Err(ApiError::bad_request(
            "settings.log_retention_invalid",
            "Log retention must keep between 1000 and 500000 records for 1 to 365 days",
        ));
    }
    if !rd_core::AUDIT_RETENTION_RECORDS_RANGE.contains(&settings.audit_retention_records)
        || !rd_core::AUDIT_RETENTION_DAYS_RANGE.contains(&settings.audit_retention_days)
    {
        return Err(ApiError::bad_request(
            "settings.audit_retention_invalid",
            "Audit retention must keep between 10000 and 2000000 records for 30 to 3650 days",
        ));
    }
    settings.otlp_endpoint = settings.otlp_endpoint.trim().to_owned();
    if !rd_core::is_valid_otlp_endpoint(&settings.otlp_endpoint)
        || !rd_core::OTLP_TIMEOUT_SECONDS_RANGE.contains(&settings.otlp_timeout_seconds)
    {
        return Err(ApiError::bad_request(
            "settings.otlp_invalid",
            "The OTLP endpoint must be an http or https URL and the timeout 1 to 60 seconds",
        ));
    }
    // Switching the export on with nowhere to send to is a setting that silently does
    // nothing, and a person who ticked the box would reasonably believe it works.
    if settings.otlp_enabled && settings.otlp_endpoint.is_empty() {
        return Err(ApiError::bad_request(
            "settings.otlp_endpoint_required",
            "Exporting traces needs an endpoint",
        ));
    }
    if settings.max_active_files == 0
        || settings.max_chunks_per_file == 0
        || settings.nntp_connections_per_file > 32
        || settings.nntp_parallel_files as usize > rd_scheduler::MAX_EXTERNAL_PARALLEL_FILES
        || settings.max_connections_per_host as usize > rd_http::MAX_CONNECTIONS_PER_HOST
    {
        return Err(ApiError::bad_request(
            "settings.concurrency_invalid",
            "Concurrency values must be valid; NNTP connections per file must be between 0 and 32, NZB files at once between 0 and 8",
        ));
    }
    settings.validate_postprocess()?;
    crate::stats_handlers::validate_stats_settings(settings)?;
    crate::hotfolder_service::validate_hotfolder_settings(settings)?;
    rd_limits::parse_timezone(&settings.bandwidth_timezone).map_err(|_| {
        ApiError::bad_request("bandwidth.timezone_invalid", "Unknown timezone")
            .with_param("timezone", &settings.bandwidth_timezone)
    })?;
    if !(rd_power::MIN_COMPLETION_COUNTDOWN..=rd_power::MAX_COMPLETION_COUNTDOWN)
        .contains(&settings.completion_countdown_seconds)
    {
        return Err(ApiError::bad_request(
            "power.countdown_invalid",
            format!(
                "The countdown must be between {} and {} seconds",
                rd_power::MIN_COMPLETION_COUNTDOWN,
                rd_power::MAX_COMPLETION_COUNTDOWN
            ),
        ));
    }
    if settings.completion_action == rd_power::CompletionAction::Script
        && settings
            .completion_script
            .as_ref()
            .is_none_or(|script| script.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "power.script_missing",
            "A completion script action needs a script name",
        ));
    }
    if !(1..=rd_core::MAX_UNKNOWN_SIZE_HEADROOM).contains(&settings.storage_unknown_size_headroom) {
        return Err(ApiError::bad_request(
            "settings.storage_headroom_invalid",
            format!(
                "The headroom factor for transfers of unknown size must be between 1 and {}",
                rd_core::MAX_UNKNOWN_SIZE_HEADROOM
            ),
        ));
    }
    settings.dlc_service_endpoint = settings
        .dlc_service_endpoint
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(endpoint) = settings.dlc_service_endpoint.as_deref()
        && !url::Url::parse(endpoint)
            .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
    {
        return Err(ApiError::bad_request(
            "dlc.endpoint_invalid",
            "The DLC decryption service must be an http or https URL",
        ));
    }
    let runtime = rd_scheduler::RuntimeSettings {
        max_active_files: settings.max_active_files as usize,
        max_chunks_per_file: settings.max_chunks_per_file as usize,
        max_connections_per_host: settings.max_connections_per_host as usize,
        external_connections_per_file: settings.nntp_connections_per_file as usize,
        external_parallel_files: settings.nntp_parallel_files as usize,
        speed_limit_bytes_per_second: settings
            .speed_limit_bytes_per_second
            .map(rd_core::ByteCount::get),
        upload_limit_bytes_per_second: settings
            .upload_limit_bytes_per_second
            .map(rd_core::ByteCount::get),
        generate_sha256: settings.generate_sha256,
        global_proxy_profile_id: settings.global_proxy_profile_id,
        custom_ca_pem: settings.custom_ca_pem.clone(),
        max_retries: settings.max_retries,
        pause_during_postprocess: settings.pause_during_postprocess,
        disabled_kinds: service_switches(settings).disabled_kinds(),
    };
    rd_scheduler::SchedulerHandle::validate_runtime_settings(&runtime)?;
    Ok(runtime)
}

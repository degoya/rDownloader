//! Validation of a settings document before it is stored.

use super::*;
use rd_api_core::input_checks::optional_text;

pub(crate) fn validate_settings(
    settings: &mut SettingsResponse,
) -> Result<rd_scheduler::RuntimeSettings, ApiError> {
    validate_access(settings)?;
    validate_display_and_telemetry(settings)?;
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
    settings.validate_update()?;
    crate::stats_handlers::validate_stats_settings(settings)?;
    crate::hotfolder_service::validate_hotfolder_settings(settings)?;
    rd_limits::parse_timezone(&settings.bandwidth_timezone).map_err(|_| {
        ApiError::bad_request("bandwidth.timezone_invalid", "Unknown timezone")
            .with_param("timezone", &settings.bandwidth_timezone)
    })?;
    validate_power_and_storage(settings)?;
    let runtime = runtime_settings(settings);
    rd_scheduler::SchedulerHandle::validate_runtime_settings(&runtime)?;
    Ok(runtime)
}

/// The proxy trust and the session limits: what decides who reaches the service, and how long.
fn validate_access(settings: &SettingsResponse) -> Result<(), ApiError> {
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
    Ok(())
}

/// How sizes are shown, how long logs and the audit trail are kept, and the trace export.
fn validate_display_and_telemetry(settings: &mut SettingsResponse) -> Result<(), ApiError> {
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
    if !rd_core::HISTORY_RETENTION_ENTRIES_RANGE.contains(&settings.history_retention_entries)
        || !rd_core::HISTORY_RETENTION_DAYS_RANGE.contains(&settings.history_retention_days)
    {
        return Err(ApiError::bad_request(
            "settings.history_retention_invalid",
            "The download history must keep between 100 and 100000 entries for 1 to 3650 days",
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
    Ok(())
}

/// The completion action and its countdown, the storage headroom and the DLC service.
fn validate_power_and_storage(settings: &mut SettingsResponse) -> Result<(), ApiError> {
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
    settings.dlc_service_endpoint = optional_text(settings.dlc_service_endpoint.take());
    if let Some(endpoint) = settings.dlc_service_endpoint.as_deref()
        && !url::Url::parse(endpoint)
            .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
    {
        return Err(ApiError::bad_request(
            "dlc.endpoint_invalid",
            "The DLC decryption service must be an http or https URL",
        ));
    }
    Ok(())
}

//! The stored service settings document, for every caller that reads it.

use crate::{ApiError, AppState, dto::SettingsResponse};

pub async fn read_settings(state: &AppState) -> Result<SettingsResponse, ApiError> {
    stored_settings(&state.database).await
}

/// The settings blob for callers that hold a database but no `AppState`, such as the
/// hotfolder watcher.
pub async fn stored_settings(database: &rd_db::Database) -> Result<SettingsResponse, ApiError> {
    // Refuses a malformed blob with `settings.invalid`, as before: this is what the settings
    // view reads, and showing silent defaults there would invite saving them back over the
    // stored configuration.
    let value = database.get_setting(rd_db::SERVICE_SETTINGS_KEY).await?;
    value
        .map(|blob| rd_db::parse_service_settings(&blob))
        .transpose()
        .map_err(|error| ApiError::bad_request("settings.invalid", error.to_string()))
        .map(Option::unwrap_or_default)
}

/// The service switches this settings document expresses.
pub fn service_switches(settings: &crate::dto::SettingsResponse) -> rd_core::ServiceSwitches {
    rd_core::ServiceSwitches {
        torrent: settings.torrent_service_enabled,
        usenet: settings.usenet_service_enabled,
        media: settings.media_service_enabled,
        gallery: settings.gallery_service_enabled,
        recording: settings.recording_service_enabled,
        remote: settings.remote_service_enabled,
    }
}

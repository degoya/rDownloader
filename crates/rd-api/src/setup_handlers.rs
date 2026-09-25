//! First-run setup wizard state.
//!
//! The wizard has no state machine of its own: completion is *derived* from what the install
//! actually has. An explicit flag is stored once the user finishes the wizard, but an install
//! that already owns a storage root counts as complete regardless — that is what keeps existing
//! installs (and anyone who abandoned the wizard after the mandatory storage step) from ever
//! seeing it again, without a migration.

use axum::{Json, extract::State};

use crate::{ApiError, AppState, dto::MessageResponse, dto::SetupStatus};

const WIZARD_COMPLETED_SETTING: &str = "setup.wizard_completed";

/// Reads the stored "user finished the wizard" flag.
async fn wizard_flag(state: &AppState) -> Result<bool, ApiError> {
    Ok(state
        .database
        .get_setting(WIZARD_COMPLETED_SETTING)
        .await?
        .and_then(|value| value.as_bool())
        .unwrap_or(false))
}

#[utoipa::path(get, path = "/api/v1/setup/status", tag = "system", responses((status = 200, body = SetupStatus)))]
pub async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let roots = state.database.list_storage_roots().await?;
    let storage_roots = roots.len() as u32;
    // Counted from the roots already loaded rather than a second query. Misconfigured, not
    // missing — the readiness card flags it without calling the install unfinished.
    let probe = rd_files::PersistenceProbe::detect();
    let ephemeral_storage_roots = roots
        .iter()
        .filter(|root| {
            probe.classify(std::path::Path::new(&root.path)) == rd_files::PathPersistence::Ephemeral
        })
        .count() as u32;
    let categories = state.database.list_categories().await?.len() as u32;
    let capture_agents = state
        .database
        .list_capture_tokens(&[rd_core::CAPTURE_SCOPE])
        .await?
        .len() as u32;
    let accounts = state.database.list_accounts().await?.len() as u32;
    let usenet_servers = state.database.list_usenet_servers().await?.len() as u32;

    Ok(Json(SetupStatus {
        // A storage root is the one mandatory outcome of the wizard, so its presence is
        // sufficient evidence that setup happened — with or without the flag.
        wizard_completed: wizard_flag(&state).await? || storage_roots > 0,
        storage_roots,
        ephemeral_storage_roots,
        categories,
        capture_agents,
        accounts,
        usenet_servers,
        // Absolute, because a storage root has to be: the configured directory may be relative
        // to the working directory, and the client cannot resolve that. Falls back to the
        // unresolved path if canonicalisation fails rather than suggesting nothing.
        suggested_storage_path: {
            let configured = state.scheduler.downloads_directory();
            dunce::canonicalize(configured)
                .unwrap_or_else(|_| configured.to_path_buf())
                .to_string_lossy()
                .into_owned()
        },
    }))
}

#[utoipa::path(post, path = "/api/v1/setup/complete", tag = "system", responses((status = 200, body = MessageResponse)))]
pub async fn complete_setup(
    State(state): State<AppState>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .set_setting(
            WIZARD_COMPLETED_SETTING.to_owned(),
            serde_json::Value::Bool(true),
        )
        .await?;
    Ok(Json(MessageResponse::new(
        "setup.wizard_completed",
        "Setup wizard completed",
    )))
}

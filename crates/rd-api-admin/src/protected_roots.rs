//! The directories no storage root may reach (security review 2026-09-28, finding 4), and the
//! refusal every way of writing a storage root shares: create and update, the settings import
//! and a full restore. A hot folder on this machine keeps out of them as well (audit
//! 2026-10-05, S9).

use std::path::{Path, PathBuf};

use rd_files::ProtectedDirectory;

use crate::dto::SettingsResponse;
use crate::{ApiError, AppState};

/// The directories no storage root may equal, contain or lie inside: where the service keeps its
/// state, runs scripts from, finds its helper binaries and loads plugins from. Gathered per
/// request, so a changed scripts or vendor setting counts from the next save on.
///
/// `incoming` is a settings document about to become the service's own — an imported bundle or
/// the settings of a restored backup. Its scripts and vendor directory count as well: a bundle
/// could otherwise name both the scripts directory and a storage root on top of it.
pub(crate) async fn protected_directories(
    state: &AppState,
    incoming: Option<&SettingsResponse>,
) -> Vec<ProtectedDirectory> {
    let mut protected = Vec::new();
    if let Some(data) = rd_core::data_directory() {
        protected.push(ProtectedDirectory::new("data", data));
    }
    match state.extraction.scripts_directory().await {
        Ok(scripts) => protected.push(ProtectedDirectory::new("scripts", scripts)),
        Err(error) => tracing::warn!(%error, "the scripts directory could not be resolved"),
    }
    if let Some(scripts) = incoming.and_then(|settings| settings.scripts_directory.as_deref()) {
        protected.push(ProtectedDirectory::new("scripts", scripts));
    }
    let program = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let configured = crate::notify_service::vendor_directory(&state.database).await;
    let named = incoming.and_then(|settings| settings.vendor_directory.clone());
    let mut vendors: Vec<PathBuf> = rd_core::vendor_directories(configured.as_deref());
    if named.is_some() {
        vendors.extend(rd_core::vendor_directories(named.as_deref()));
    }
    for vendor in vendors {
        protected.push(if program.as_ref() == Some(&vendor) {
            ProtectedDirectory::new("program", vendor).top_level_only()
        } else {
            ProtectedDirectory::new("vendor", vendor)
        });
    }
    protected.push(ProtectedDirectory::new("tools", state.tools.root()));
    protected.push(ProtectedDirectory::new("plugins", state.plugins.root()));
    protected
}

/// Refuses a root that would reach one of [`protected_directories`].
pub(crate) fn refuse_protected(
    path: &Path,
    protected: &[ProtectedDirectory],
) -> Result<(), ApiError> {
    match rd_files::protected_collision(path, protected) {
        None => Ok(()),
        Some(directory) => Err(ApiError::bad_request(
            "storage_root.protected_directory",
            "A storage root may not be, contain or lie inside a directory the service runs \
             programs, scripts or plugins from or keeps its data in",
        )
        .with_param("path", path.display())
        .with_param("directory", directory.kind)),
    }
}

/// Refuses a hot folder that would reach one of [`protected_directories`] (audit 2026-10-05,
/// S9). A hot folder is created when it does not exist, takes the files that land in it and
/// moves them into its processed and failed folders: on the data or the plugin directory it
/// would carry away the service's own files.
pub(crate) fn refuse_protected_hotfolder(
    path: &Path,
    protected: &[ProtectedDirectory],
) -> Result<(), ApiError> {
    match rd_files::protected_collision(path, protected) {
        None => Ok(()),
        Some(directory) => Err(ApiError::bad_request(
            "hotfolder.protected_directory",
            "A hotfolder may not be, contain or lie inside a directory the service runs \
             programs, scripts or plugins from or keeps its data in",
        )
        .with_param("path", path.display())
        .with_param("directory", directory.kind)),
    }
}

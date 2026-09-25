//! Gallery provider: image galleries fetched through external `gallery-dl`. One queue row
//! covers a whole gallery URL; the files land in a subfolder of the package destination.

mod runner;

use std::sync::Arc;

use anyhow::Result;
use rd_core::GallerySettings;
use rd_db::Database;
use rd_scheduler::ExternalRunner;
use tokio::sync::RwLock;

pub use runner::GalleryRunner;

/// Settings shared between the runner and the settings endpoint.
pub type SharedGallerySettings = Arc<RwLock<GallerySettings>>;

/// Reads the gallery settings from the `service.settings` blob.
///
/// Refuses a malformed blob rather than running on defaults: this is read once at start-up,
/// so an unusable configuration must stop the service instead of silently disabling the
/// tool options the person configured.
pub async fn load_gallery_settings(database: &Database) -> Result<GallerySettings> {
    database.service_settings().await
}

/// Creates the shared settings handle from the database.
pub async fn shared_settings(database: &Database) -> Result<SharedGallerySettings> {
    Ok(Arc::new(RwLock::new(
        load_gallery_settings(database).await?,
    )))
}

/// Runner wired to the shared settings handle.
pub fn build(database: Database, settings: SharedGallerySettings) -> Arc<dyn ExternalRunner> {
    Arc::new(GalleryRunner::new(database, settings))
}

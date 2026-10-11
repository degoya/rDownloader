//! Gallery provider: image galleries fetched through external `gallery-dl`. One queue row
//! covers a whole gallery URL; the files land in a subfolder of the package destination.

#![warn(unreachable_pub)]

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
/// Read once at start-up, field by field (owner, 2026-10-04, RA-DB-02): a value that does not
/// parse reads as its default with a warning naming it, instead of refusing the start; only
/// the scheduler's runtime values refuse one.
pub async fn load_gallery_settings(database: &Database) -> Result<GallerySettings> {
    database.service_settings_per_field().await
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

/// [`build`], with gallery-dl handed each download's proxy and the custom CA (RD-1240-08).
pub fn build_with_tool_network(
    database: Database,
    settings: SharedGallerySettings,
    network: rd_scheduler::ToolNetworkSource,
) -> Arc<dyn ExternalRunner> {
    Arc::new(GalleryRunner::new(database, settings).with_tool_network(network))
}

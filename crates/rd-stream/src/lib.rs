//! Livestream recording via external `streamlink`: an [`ExternalRunner`] that writes the
//! stream to disk until it ends or is stopped, plus the liveness probe the channel monitor
//! uses. Recordings are exempt from the global active-file cap.

#![warn(unreachable_pub)]

mod probe;
mod runner;
mod schedule;
mod segments;
mod sidecars;

use std::sync::Arc;

use anyhow::Result;
use rd_core::StreamSettings;
use rd_db::Database;
use rd_scheduler::ExternalRunner;
use tokio::sync::RwLock;

pub use probe::{
    StreamProbe, parse_probe_output, parse_replay_capability, probe_live, probe_stream,
};
pub use runner::StreamRunner;
pub use schedule::{Occurrence, is_watching, occurrences, validate as validate_schedule};
pub use segments::{
    SEGMENT_EXTENSION, SegmentOutcome, SegmentTool, end_reason, next_name, should_continue,
};
pub use sidecars::{
    SidecarClients, capture as capture_sidecars, thumbnail_extension, thumbnail_url,
};

/// Settings shared between the runner, the monitor and the settings endpoint.
pub type SharedStreamSettings = Arc<RwLock<StreamSettings>>;

/// Reads the recording settings from the `service.settings` blob.
///
/// Read once at start-up, field by field (owner, 2026-10-04, RA-DB-02): a value that does not
/// parse reads as its default with a warning naming it, instead of refusing the start; only
/// the scheduler's runtime values refuse one.
pub async fn load_stream_settings(database: &Database) -> Result<StreamSettings> {
    database.service_settings_per_field().await
}

/// Creates the shared settings handle from the database.
pub async fn shared_settings(database: &Database) -> Result<SharedStreamSettings> {
    Ok(Arc::new(RwLock::new(load_stream_settings(database).await?)))
}

/// Runner wired to the shared settings handle.
///
/// Sidecar fetches then trust the platform store alone. Prefer
/// [`build_with_network_defaults`] wherever the scheduler's handle is in reach.
pub fn build(database: Database, settings: SharedStreamSettings) -> Arc<dyn ExternalRunner> {
    Arc::new(StreamRunner::new(database, settings))
}

/// [`build`], with the sidecar fetches trusting what the rest of the service trusts, and
/// streamlink handed each recording's proxy and the custom CA (RD-1240-08).
pub fn build_with_network_defaults(
    database: Database,
    settings: SharedStreamSettings,
    network: rd_http::SharedNetworkDefaults,
    tools: rd_scheduler::ToolNetworkSource,
) -> Arc<dyn ExternalRunner> {
    Arc::new(
        StreamRunner::new(database, settings)
            .with_network_defaults(network)
            .with_tool_network(tools),
    )
}

/// streamlink executable: explicit setting → managed store → vendor folders and `PATH` → the
/// portable Windows build shipped as `vendor/streamlink/bin/streamlink(.exe)`.
#[must_use]
pub fn locate_streamlink(settings: &StreamSettings) -> Option<rd_core::ResolvedTool> {
    lease_streamlink(settings).map(|(tool, _)| tool)
}

/// [`locate_streamlink`], keeping the lease a caller that is about to run the binary holds.
///
/// A managed streamlink version is not removed while a lease on it is alive, so a recording
/// that is already running keeps the binary it started with (RD-102-02).
#[must_use]
pub fn lease_streamlink(
    settings: &StreamSettings,
) -> Option<(rd_core::ResolvedTool, Option<rd_core::ToolLease>)> {
    let vendor = settings.vendor_directory.as_deref();
    if let Some(found) = rd_core::locate_tool_leased(
        settings.record_streamlink_executable.as_deref(),
        vendor,
        "streamlink",
    ) {
        return Some(found);
    }
    // The portable Windows layout is a vendor folder in a different shape, never a managed
    // version, so it carries no lease.
    rd_core::vendor_directories(vendor)
        .into_iter()
        .find_map(|directory| {
            let path =
                rd_core::executable_in(&directory.join("streamlink").join("bin"), "streamlink")?;
            Some((
                rd_core::ResolvedTool {
                    path,
                    source: rd_core::ToolSource::Vendor,
                },
                None,
            ))
        })
}

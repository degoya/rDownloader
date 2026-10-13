//! Parameters of the torrent tools: the views of one torrent, its trackers, its seeding
//! override and the engine's views. Split from [`super::params_handling`], which re-exports
//! them, so neither file outgrows the 500-line rule (RD-1280-08).

use rmcp::schemars;
use serde::Deserialize;

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TorrentView {
    /// Files, sizes, progress and the plan.
    Summary,
    /// Connected peers, a page at a time.
    Peers,
    /// Piece availability.
    Pieces,
    /// Aggregate transfer figures.
    Stats,
    /// Trackers and their announce state.
    Trackers,
    /// The seeding policy in force, and where each value comes from.
    Seeding,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct TorrentViewParams {
    /// The download id of the torrent.
    pub id: String,
    pub view: TorrentView,
    /// Peers only: page size.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Peers only: the cursor from the previous page.
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TrackerAction {
    /// Replace the tracker list with `trackers`.
    Set,
    /// Announce to every tracker now.
    Reannounce,
    /// Ask every tracker for seeder and leecher counts.
    Scrape,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct TrackersParams {
    pub id: String,
    pub action: TrackerAction,
    /// For `set`: the complete list, each `{url, tier}` as PUT …/torrent/trackers documents it.
    #[serde(default)]
    pub trackers: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SeedingParams {
    pub id: String,
    /// Drop the override so the value is inherited again.
    #[serde(default)]
    pub clear: Option<bool>,
    /// The override: `enabled`, `ratio`, `time_minutes`, `time_unlimited`.
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EngineView {
    /// What the torrent engine supports.
    Capabilities,
    /// Listening port, bound interface and reachability.
    NetworkStatus,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct EngineViewParams {
    pub view: EngineView,
}

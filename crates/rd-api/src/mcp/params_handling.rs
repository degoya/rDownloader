//! Parameters for the tools RD-120-32 added: the LinkGrabber's candidate level, queue
//! handling, torrents, post-processing, managed tools, storage and site-rule writing.
//!
//! Same arrangement as [`super::params_config`]: string-typed mirrors of the REST bodies, turned
//! into the REST body by [`body`] and deserialised by the REST type itself, so the handler sees
//! exactly what a browser would have sent and answers with the same codes. Where a REST body is
//! a tree the interface builds — a torrent file plan, media criteria, a site rule — the tool
//! takes it as `body` and says in its description which route documents it.

use rmcp::schemars;
use serde::Deserialize;

use crate::ApiError;

/// Builds a REST request body from a tool's arguments, through the same credential screen the
/// `definition` passthroughs use.
pub(crate) fn body<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<T, ApiError> {
    match value {
        serde_json::Value::Object(map) => super::error::from_definition(map),
        other => serde_json::from_value(other)
            .map_err(|error| ApiError::bad_request("request.body_invalid", error.to_string())),
    }
}

/// A REST answer with every `password` key taken out, at any depth.
///
/// The package rows carry their archive password in clear, on purpose (RD-104-04: it is public
/// already, from a release title or a `{{password}}` marker). The existing tools never repeated
/// it — `list_collector` and `list_packages` answer with `has_password` — and the tools that
/// answer with a whole row follow them rather than start.
pub(crate) fn public<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, ApiError> {
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.remove("password");
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(value)
        .map_err(|error| ApiError::bad_request("request.body_invalid", error.to_string()))?;
    strip(&mut value);
    Ok(value)
}

/// An id plus a REST body handed through.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct IdBodyParams {
    pub id: String,
    /// The request body, exactly as the matching REST endpoint documents it.
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

/// A list of ids.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct IdsParams {
    pub ids: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ListCandidatesParams {
    /// Only the links of this LinkGrabber package.
    #[serde(default)]
    pub package_id: Option<String>,
    /// Only the links of this intake batch.
    #[serde(default)]
    pub batch_id: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct MoveCandidatesParams {
    pub ids: Vec<String>,
    /// The LinkGrabber package to move them into.
    #[serde(default)]
    pub package_id: Option<String>,
    /// Or: the name of a new package to create for them.
    #[serde(default)]
    pub new_package_name: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ReorderMembersParams {
    /// The package whose members are ordered.
    pub package_id: String,
    /// Every member of that package, each once, in the new order.
    pub ids: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateCandidateParams {
    pub id: String,
    /// The file name to download under.
    #[serde(default)]
    pub file_name: Option<String>,
    /// For a media link: the variant to fetch, as get_candidate_details view=media lists them.
    #[serde(default)]
    pub media_variant: Option<String>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateView {
    /// The formats and presets of a media link.
    Media,
    /// The files of a directory-listing link and which are excluded.
    Listing,
    /// The files, sizes and plan of a torrent or magnet link.
    Torrent,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CandidateViewParams {
    pub id: String,
    pub view: CandidateView,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CandidatePlanParams {
    pub id: String,
    /// Which plan: `media` (PUT /api/v1/collector/candidates/{id}/media/selection: `preset` or
    /// `criteria`), `listing` (PUT …/listing/plan: `excluded` paths) or `torrent`
    /// (PUT …/torrent/plan: `included`, `excluded`, `priorities`, `exclusion_patterns`,
    /// `sequential`).
    pub kind: CandidateView,
    /// The REST body of that route.
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MediaPreviewKind {
    /// Which format a `preset` or `criteria` would pick.
    Selection,
    /// What file name an output `template` would expand to.
    Output,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct MediaPreviewParams {
    pub id: String,
    pub kind: MediaPreviewKind,
    /// `{preset}` or `{criteria}` for selection; `{template}` for output.
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MirrorAction {
    /// Make this link its group's chosen mirror, above the standing preference.
    Pin,
    /// Release a pin, so the preference chooses again.
    Release,
    /// Take a proposed mirror group apart.
    Dissolve,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct MirrorParams {
    /// A link of the mirror group.
    pub id: String,
    pub action: MirrorAction,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct MirrorPreferenceParams {
    /// Preferred quality token, e.g. `1080p`. Absent or empty clears it.
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub hoster: Option<String>,
    /// Hosters the LinkGrabber hides (RD-130-21). Absent keeps the stored list, unlike the
    /// three facets: a call about the quality must not bring back what somebody hid. An empty
    /// list shows every hoster again.
    #[serde(default)]
    pub hidden_hosters: Option<Vec<String>>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EntryKindParam {
    /// A LinkGrabber package.
    Collector,
    /// An NZB import.
    Nzb,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct EntryRefParam {
    pub kind: EntryKindParam,
    pub id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ReorderCollectorParams {
    /// The entries that move, in their new order.
    pub entries: Vec<EntryRefParam>,
    /// The entry they are placed behind; absent places them at the top.
    #[serde(default)]
    pub after: Option<EntryRefParam>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct EnqueueNzbParams {
    pub id: String,
    /// Create the downloads paused.
    #[serde(default)]
    pub paused: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct RenameParams {
    pub id: String,
    /// The new name.
    pub name: String,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClearScopeParam {
    /// Packages in which every file succeeded.
    Completed,
    /// Packages holding a failed or blocked file with nothing left to do.
    Failed,
    /// Every package that is not working any more.
    All,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ClearPackagesParams {
    pub scope: ClearScopeParam,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ExtractPackagesParams {
    pub ids: Vec<String>,
    /// Unpack again even where the last attempt finished or failed.
    #[serde(default)]
    pub force: Option<bool>,
}

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

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PostprocessOptions {
    /// User scripts a category or package may run.
    Scripts,
    /// Post-processing steps installed plugins provide.
    PluginSteps,
    /// Upload destinations installed plugins provide.
    UploadDestinations,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct PostprocessOptionsParams {
    pub kind: PostprocessOptions,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManagedToolsView {
    /// Every managed tool, its versions and which is active.
    Tools,
    /// Whether yt-dlp and ffmpeg are usable, and from where.
    Media,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ManagedToolsParams {
    #[serde(default)]
    pub view: Option<ManagedToolsView>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManageAction {
    /// Download and verify a version from the signed manifest.
    Install,
    /// Make an installed version the one in use.
    Activate,
    /// Go back to the version active before.
    Rollback,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ManageToolParams {
    /// The tool's name, as list_managed_tools gives it.
    pub name: String,
    pub action: ManageAction,
    /// install and activate: the version; absent means the manifest's current one.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct StorageTargetParams {
    /// A storage root id from get_storage_capacity, or `fallback`.
    pub target: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SiteRuleParams {
    /// The rule document, as GET /api/v1/site-rules shows your own rules under `rule`.
    pub rule: serde_json::Value,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateSiteRuleParams {
    pub id: String,
    /// The whole rule document; this replaces the stored one.
    pub rule: serde_json::Value,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct TestSiteRuleParams {
    /// The rule document to try.
    pub rule: serde_json::Value,
    /// The page address to try it against.
    pub address: String,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NzbView {
    /// The files and their segment state.
    Files,
    /// The post-processing steps planned or run.
    Postprocess,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct NzbViewParams {
    pub id: String,
    pub view: NzbView,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ReorderPackagesParams {
    /// Download package ids (from list_packages) in their new order.
    pub ids: Vec<String>,
}

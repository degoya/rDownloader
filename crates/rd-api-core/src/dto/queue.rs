//! The download queue: direct downloads, list paging, summaries and rates, packages and files.

use super::*;

/// Direct URL queue request.
#[derive(Deserialize, ToSchema)]
pub struct CreateDownloadRequest {
    #[schema(format = "uri")]
    pub url: String,
    pub package_name: Option<String>,
    pub file_name: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    pub account_id: Option<rd_core::AccountId>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub priority: Option<rd_core::DownloadPriority>,
    /// Create the download paused instead of queued (API-09). The row is written in that
    /// state, so the scheduler never gets the chance to start it first; resume it as any
    /// other paused download. Absent means `false`.
    #[serde(default)]
    pub paused: bool,
}

/// The optional page window of a growing list (API-15): downloads, packages, LinkGrabber
/// batches, links and packages, NZB imports.
///
/// Without either parameter the list comes back whole and unchanged. With one, the answer is
/// the same array cut from the list's own order, and the `X-Total-Count` header names the
/// length of the whole list; an offset past the end is an empty page. A `limit` outside
/// 1 to [`crate::list_bounds::MAX_PAGE_LIMIT`], or a value that is no number, is refused as
/// `request.page_limit` (read by [`crate::list_bounds::Page`]) rather than clamped, so a client never mistakes a shortened page for the end of the list. The rows are
/// still read whole and sliced in the handler: the answer is bounded, the load is not.
#[derive(Clone, Copy, Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// Rows to return at most, 1 to 1000. With `limit` or `offset` set the answer carries
    /// `X-Total-Count`.
    pub limit: Option<u32>,
    /// Rows to skip first; alone it returns everything after them.
    pub offset: Option<u32>,
}

/// Aggregated queue counters shown below the download list.
#[derive(Serialize, ToSchema)]
pub struct DownloadSummaryResponse {
    pub queued: u32,
    pub active: u32,
    pub paused: u32,
    pub blocked: u32,
    pub failed: u32,
    pub completed: u32,
    pub total_bytes: rd_core::ByteCount,
    pub committed_bytes: rd_core::ByteCount,
    /// Everything not yet fetched, including entries that are paused, blocked or already
    /// downloaded and now being verified, repaired, unpacked or seeded.
    ///
    /// Deliberately wider than `transferring_remaining_bytes`: this is "how much is still
    /// outstanding", not "how much is on its way". The remaining time is built from the
    /// narrower figure, and the interface names the difference rather than blurring it.
    pub remaining_bytes: rd_core::ByteCount,
    /// Bytes still to fetch across the entries that are actually going to be fetched at the
    /// current rate — queued, waiting to retry, resolving or downloading.
    ///
    /// `None` while one of those entries has no known size, because the sum would then be a
    /// lower bound rather than the remainder.
    pub transferring_remaining_bytes: Option<rd_core::ByteCount>,
    /// Combined smoothed transfer rate of the whole queue, in bytes per second.
    pub bytes_per_second: u64,
    /// Seconds until the queue is through, at the current rate.
    ///
    /// `None` whenever a number would be invented rather than measured: nothing is moving, or
    /// one of the entries still to be fetched has no known size, which would make the figure a
    /// lower bound presented as an answer.
    pub eta_seconds: Option<u64>,
    pub storage: Vec<StorageSpace>,
}

/// One entry's live rate and the remaining time it implies.
#[derive(Serialize, ToSchema)]
pub struct DownloadRateEntry {
    pub id: rd_core::DownloadId,
    pub bytes_per_second: u64,
    /// `None` when the size is unknown or the entry is not moving.
    pub eta_seconds: Option<u64>,
}

/// Live transfer rates: the queue as a whole, every entry that is moving, and every queued
/// entry that waits for a connection to its host.
///
/// Entries at rest are left out — their rate is zero and their remaining time is nothing, and
/// saying so for every finished download in a long list is pure payload.
#[derive(Serialize, ToSchema)]
pub struct DownloadRatesResponse {
    pub bytes_per_second: u64,
    /// Bytes still to fetch across queued, retrying, resolving and downloading entries;
    /// `None` while one of them has no known size.
    pub transferring_remaining_bytes: Option<rd_core::ByteCount>,
    /// Seconds until the queue is through, or `None` when no honest figure exists.
    pub eta_seconds: Option<u64>,
    pub downloads: Vec<DownloadRateEntry>,
    /// Queued entries held back because their host has no free connection (RD-1130-02).
    /// They take no place among the files running at once meanwhile.
    pub waiting_for_host: Vec<DownloadHostWait>,
}

/// A queued entry waiting for a connection to its host.
#[derive(Serialize, ToSchema)]
pub struct DownloadHostWait {
    pub id: rd_core::DownloadId,
    /// The host as the per-host connection limit counts it: lower case, no `www.`, no port.
    pub host: String,
}

/// Figures only, for the desktop tray.
///
/// Deliberately not `DownloadSummaryResponse`: that one carries storage entries with their
/// paths, and this is served to the capture agent, whose token is scoped for handing links in.
/// The event stream is filtered to intake for the same reason — the full bus would carry
/// download paths and account names. Counts and byte totals say enough for an icon and a
/// tooltip and say nothing about what is being downloaded.
#[derive(Serialize, ToSchema)]
pub struct CaptureSummaryResponse {
    pub active: u32,
    pub queued: u32,
    pub failed: u32,
    /// Bytes committed across everything not finished, and the total where it is known.
    pub committed_bytes: rd_core::ByteCount,
    pub total_bytes: rd_core::ByteCount,
    /// The queue's smoothed transfer rate, in bytes per second.
    ///
    /// Served here so the tray shows the same figure as the web interface. The agent used to
    /// derive its own from two consecutive reads, unsmoothed, which meant two different
    /// answers to the same question; the service keeps the rate now (RD-104-02).
    pub bytes_per_second: u64,
    /// Seconds until the queue is through at that rate, or `null` when no honest figure
    /// exists: an entry still to be fetched whose size is unknown, a rate of zero, a paused
    /// transfer. The same rule the web interface follows, because it is the same number.
    ///
    /// Taken from the very `queue_rate()` value the rate above comes from (RD-108-01). The
    /// estimate was already being computed there and then dropped, so the tray had the rate
    /// but not the time it implies; a second formula here would have been a second answer to
    /// one question, which is the mistake RD-104-02 exists to have ended.
    pub eta_seconds: Option<u64>,
    /// Files paused, by a person or by a pause of the whole queue: what the tray's "resume all"
    /// would queue again (RD-1100-06).
    pub paused: u32,
    /// When the timed pause of the whole queue ends, while one holds (RD-190-20); the tray says
    /// "paused until" with it.
    pub paused_until: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether the token asking may pause and resume the queue (`capture:queue`, chosen when the
    /// agent was paired). The tray offers the two entries only when it may (RD-1100-06).
    pub queue_control: bool,
}

/// New name for a package **and** for the folder its files live in (RD-106-13).
#[derive(Deserialize, ToSchema)]
pub struct PackageFolderRequest {
    /// New name (1–200 characters), sanitized into a folder name by the same rules a file
    /// rename uses. A folder of that name that already exists is refused, not avoided.
    pub name: String,
}

/// Category and/or priority change for one package.
#[derive(Deserialize, ToSchema)]
pub struct PackageUpdateRequest {
    pub category_id: Option<rd_core::CategoryId>,
    /// Removes the category (default destination) when `true`.
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
    /// New display name (1–200 characters).
    pub name: Option<String>,
    /// Archive password used for extraction.
    ///
    /// Readable again on the package (RD-104-04); see `DownloadPackage::password`.
    pub password: Option<String>,
    /// Removes the stored archive password when `true`.
    #[serde(default)]
    pub clear_password: bool,
    /// Explicit post-processing level; see `clear_postprocess_level` to inherit again.
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub clear_postprocess_level: bool,
    /// Post-processing script file name inside the scripts directory.
    pub script: Option<String>,
    #[serde(default)]
    pub clear_script: bool,
}

/// Packages to extract manually.
#[derive(Deserialize, ToSchema)]
pub struct PackageExtractRequest {
    pub ids: Vec<rd_core::PackageId>,
}

/// Packages to remove from the download list.
///
/// `force` is the difference between tidying up and throwing work away: without it a package
/// whose files are still running, waiting or seeding is refused, because removing it cancels
/// those files and deletes what they had already written.
#[derive(Deserialize, ToSchema)]
pub struct PackageDeleteRequest {
    pub ids: Vec<rd_core::PackageId>,
    /// Cancels running files and removes the package anyway.
    #[serde(default)]
    pub force: bool,
}

/// Which finished packages the "clear the list" action should remove.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageClearScope {
    /// Packages in which every file succeeded.
    Completed,
    /// Packages that hold a failed or blocked file and have nothing left to do.
    Failed,
    /// Every package that is not working any more, whatever the outcome.
    All,
    /// Every package, running ones included: what still runs or waits is cancelled and seeding
    /// is stopped before the packages go. Only a package being post-processed is left alone.
    /// Requires `confirmed`.
    Everything,
}

/// Bulk removal of packages from the download list.
#[derive(Deserialize, ToSchema)]
pub struct PackageClearRequest {
    pub scope: PackageClearScope,
    /// Also deletes what the unfinished files of the removed packages had written outside
    /// staging: tool fragments beside the target and an unfinished torrent's data. Incomplete
    /// staging files go with every removal; finished files always stay.
    #[serde(default)]
    pub delete_partial: bool,
    /// The confirmation `everything` must carry; the server refuses that scope without it.
    #[serde(default)]
    pub confirmed: bool,
}

/// A package the clear pass deliberately left alone, and the stable code saying why.
#[derive(Serialize, ToSchema)]
pub struct PackageClearSkip {
    pub package_id: rd_core::PackageId,
    /// The package name, so the reader can find it without looking up the id.
    pub name: String,
    /// `package.members_active`, `package.members_seeding`, `package.postprocess_running`
    /// or `package.members_unfinished`.
    pub code: String,
}

/// What a clear pass did: whole packages removed, and the ones it refused to touch.
#[derive(Serialize, ToSchema)]
pub struct PackageClearResponse {
    pub removed: usize,
    pub skipped: Vec<PackageClearSkip>,
}

/// New file name for a queued, paused or failed download.
#[derive(Deserialize, ToSchema)]
pub struct DownloadRenameRequest {
    pub file_name: String,
}

/// Category and/or priority change for several packages.
#[derive(Deserialize, ToSchema)]
pub struct PackageBulkRequest {
    pub ids: Vec<rd_core::PackageId>,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub clear_postprocess_level: bool,
    pub script: Option<String>,
    #[serde(default)]
    pub clear_script: bool,
}

/// Complete queue order; packages are positioned in the given sequence.
#[derive(Deserialize, ToSchema)]
pub struct PackageReorderRequest {
    pub ids: Vec<rd_core::PackageId>,
}

/// Complete file order of one package; the ids have to be exactly its files, each once.
#[derive(Deserialize, ToSchema)]
pub struct DownloadReorderRequest {
    pub package_id: rd_core::PackageId,
    pub ids: Vec<rd_core::DownloadId>,
}

/// Bulk action applied to individual files.
#[derive(Clone, Copy, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadBulkAction {
    Pause,
    Resume,
    Cancel,
    Remove,
    /// Discards partial data and checkpoints and queues the file again from zero; a finished
    /// payload is left on disk, so the fresh attempt lands beside it under a free name.
    Reset,
    /// Like `Reset`, and deletes the finished payload as well.
    ResetDeleteFiles,
}

#[derive(Deserialize, ToSchema)]
pub struct DownloadBulkRequest {
    pub ids: Vec<rd_core::DownloadId>,
    pub action: DownloadBulkAction,
}

#[derive(Serialize, ToSchema)]
pub struct DownloadBulkResponse {
    pub affected: u32,
    pub errors: Vec<String>,
    /// The same refusals as `errors`, coded and in the same order, for the interface to
    /// translate; `errors` stays for the clients that read the English text.
    pub refusals: Vec<MessageResponse>,
}

/// Options for resetting a single file.
#[derive(Deserialize, ToSchema)]
pub struct DownloadResetRequest {
    /// Delete a finished payload as well. Off by default: a reset that keeps the file lets the
    /// fresh attempt land beside it instead of destroying the only copy.
    #[serde(default)]
    pub delete_completed_files: bool,
}

/// Files whose packages should be extracted.
#[derive(Deserialize, ToSchema)]
pub struct DownloadExtractRequest {
    pub ids: Vec<rd_core::DownloadId>,
}

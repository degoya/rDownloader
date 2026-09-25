//! Parameter structs and compact output projections for the MCP tools.
//!
//! Lists are always projected and paginated so tool results stay readable for
//! an LLM client; raw domain rows are never dumped wholesale.

use chrono::{DateTime, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_LIMIT: usize = 50;
pub(crate) const MAX_LIMIT: usize = 200;

/// A window into a larger list plus enough metadata to page further.
#[derive(Serialize)]
pub(crate) struct PagedList<T: Serialize> {
    pub items: Vec<T>,
    /// Matching items before `limit`/`offset` were applied.
    pub total: usize,
    /// `true` when more items exist beyond this window.
    pub truncated: bool,
}

/// Applies offset/limit to an already filtered list and projects each row.
pub(crate) fn paginate<T, U: Serialize>(
    rows: Vec<T>,
    limit: Option<u32>,
    offset: Option<u32>,
    project: impl Fn(T) -> U,
) -> PagedList<U> {
    let total = rows.len();
    let offset = offset.unwrap_or(0) as usize;
    let limit = (limit.unwrap_or(DEFAULT_LIMIT as u32) as usize).clamp(1, MAX_LIMIT);
    let items: Vec<U> = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(project)
        .collect();
    let truncated = offset + items.len() < total;
    PagedList {
        items,
        total,
        truncated,
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PriorityParam {
    Low,
    Normal,
    High,
}

impl From<PriorityParam> for rd_core::DownloadPriority {
    fn from(value: PriorityParam) -> Self {
        match value {
            PriorityParam::Low => Self::Low,
            PriorityParam::Normal => Self::Normal,
            PriorityParam::High => Self::High,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DownloadActionParam {
    Pause,
    Resume,
    Cancel,
    /// Cancels an active file first, then removes it from the list.
    Remove,
    /// Discards partial data and checkpoints and downloads the file again from the start. A
    /// finished file is kept, so the new attempt lands beside it under a free name.
    Reset,
    /// Like `reset`, and deletes the finished file as well. Irreversible.
    ResetDeleteFiles,
}

impl From<DownloadActionParam> for crate::dto::DownloadBulkAction {
    fn from(value: DownloadActionParam) -> Self {
        match value {
            DownloadActionParam::Pause => Self::Pause,
            DownloadActionParam::Resume => Self::Resume,
            DownloadActionParam::Cancel => Self::Cancel,
            DownloadActionParam::Remove => Self::Remove,
            DownloadActionParam::Reset => Self::Reset,
            DownloadActionParam::ResetDeleteFiles => Self::ResetDeleteFiles,
        }
    }
}

/// Groups download states the same way the status summary does.
#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StateFilter {
    Queued,
    Active,
    Paused,
    Blocked,
    Failed,
    Completed,
}

impl StateFilter {
    pub(crate) fn matches(self, state: rd_core::DownloadState) -> bool {
        use rd_core::DownloadState as S;
        match self {
            Self::Queued => matches!(state, S::Queued | S::RetryWait),
            Self::Active => matches!(
                state,
                S::Resolving | S::Downloading | S::Verifying | S::Repairing | S::Extracting
            ),
            Self::Paused => state == S::Paused,
            Self::Blocked => state == S::Blocked,
            Self::Failed => matches!(state, S::Failed | S::Cancelled),
            Self::Completed => state == S::Completed,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigSection {
    Categories,
    StorageRoots,
    Accounts,
    ProxyProfiles,
    Providers,
    Plugins,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct AddDownloadsParams {
    /// Direct HTTP(S) or magnet URLs (1-50). For hoster links use collect_links instead.
    pub urls: Vec<String>,
    /// Package (folder) name shared by all added files; derived from the URL when omitted.
    pub package_name: Option<String>,
    /// Target category id; the default storage root is used when omitted.
    pub category_id: Option<String>,
    /// Provider account id to download with; auto-selected when omitted.
    pub account_id: Option<String>,
    pub priority: Option<PriorityParam>,
    /// Create the downloads in paused state instead of starting immediately.
    pub start_paused: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ListDownloadsParams {
    /// Filter by state group.
    pub state: Option<StateFilter>,
    /// Only files belonging to this package id.
    pub package_id: Option<String>,
    /// Case-insensitive substring match on the file name.
    pub name_contains: Option<String>,
    /// Page size (default 50, max 200).
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct GetDownloadParams {
    pub id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ControlDownloadsParams {
    pub action: DownloadActionParam,
    /// Download ids to act on (1-500). `remove` on active files can take a few seconds each.
    pub ids: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ListPackagesParams {
    /// Page size (default 50, max 200).
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DeletePackagesParams {
    /// Package ids to remove including all their files (1-500).
    pub ids: Vec<String>,
    /// Cancel files that are still running and remove the package anyway. Without this a
    /// package that is still working is refused instead of being thrown away.
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CollectLinksParams {
    /// Free text or URLs; every HTTP(S)/magnet link found is analyzed by the LinkGrabber.
    pub text: String,
    /// Explicit package name keeping all links together.
    pub package_name: Option<String>,
    /// Archive password announced with the links.
    pub password: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CheckLinksParams {
    /// Candidate ids to check; alternatively pass batch_id to check a whole batch.
    pub candidate_ids: Option<Vec<String>>,
    /// Check every open candidate of this collector batch.
    pub batch_id: Option<String>,
    /// How long to wait for results before returning a snapshot (default 10, max 30).
    pub wait_seconds: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ListCollectorParams {
    /// Only packages from this batch id.
    pub batch_id: Option<String>,
    /// Page size over collector packages (default 50, max 200).
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct EnqueueCollectorParams {
    /// Collector package ids to move into the download list (1-500).
    pub package_ids: Vec<String>,
    /// Create the downloads paused instead of starting immediately.
    pub start_paused: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ListAutomationsParams {
    /// Include the run history of each automation, newest first.
    pub with_runs: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ToggleAutomationParams {
    /// Automation id.
    pub id: String,
    /// Whether the automation should start new runs.
    pub enabled: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct GetSettingsParams {
    /// Optional projection: return only these top-level settings keys.
    pub keys: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateSettingsParams {
    /// Partial settings object; unknown top-level keys are rejected. The merged
    /// result is validated and applied live.
    pub patch: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ListConfigurationParams {
    pub section: ConfigSection,
}

/// Compact projection of one download file.
#[derive(Serialize)]
pub(crate) struct DownloadItem {
    pub id: String,
    pub package_id: String,
    pub file_name: String,
    pub state: rd_core::DownloadState,
    /// Percentage 0-100 when the total size is known.
    pub progress_percent: Option<u8>,
    pub total_bytes: Option<u64>,
    pub committed_bytes: u64,
    pub retry_count: u32,
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl From<rd_core::DownloadFile> for DownloadItem {
    fn from(file: rd_core::DownloadFile) -> Self {
        let total = file.total_bytes.map(rd_core::ByteCount::get);
        let committed = file.committed_bytes.get();
        let progress_percent = total.filter(|total| *total > 0).map(|total| {
            u8::try_from(committed.saturating_mul(100) / total)
                .unwrap_or(100)
                .min(100)
        });
        Self {
            id: file.id.to_string(),
            package_id: file.package_id.to_string(),
            file_name: file.file_name,
            state: file.state,
            progress_percent,
            total_bytes: total,
            committed_bytes: committed,
            retry_count: file.retry_count,
            error: file.last_error.map(|failure| failure.message),
            updated_at: file.updated_at,
        }
    }
}

/// Compact projection of one download package.
#[derive(Serialize)]
pub(crate) struct PackageItem {
    pub id: String,
    pub name: String,
    pub state: rd_core::PackageState,
    pub destination: String,
    pub category_id: Option<String>,
    pub priority: rd_core::DownloadPriority,
    pub kind: rd_core::DownloadKind,
}

impl From<rd_core::DownloadPackage> for PackageItem {
    fn from(package: rd_core::DownloadPackage) -> Self {
        Self {
            id: package.id.to_string(),
            name: package.name,
            state: package.state,
            destination: package.destination,
            category_id: package.category_id.map(|id| id.to_string()),
            priority: package.priority,
            kind: package.kind,
        }
    }
}

/// Compact projection of one LinkGrabber candidate.
#[derive(Serialize)]
pub(crate) struct CandidateItem {
    pub id: String,
    pub package_id: Option<String>,
    pub url: String,
    pub state: rd_core::LinkCandidateState,
    pub file_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub provider: Option<String>,
    pub error: Option<String>,
}

impl From<rd_core::LinkCandidate> for CandidateItem {
    fn from(candidate: rd_core::LinkCandidate) -> Self {
        Self {
            id: candidate.id.to_string(),
            package_id: candidate.package_id.map(|id| id.to_string()),
            url: candidate.url.to_string(),
            state: candidate.state,
            file_name: candidate.file_name,
            size_bytes: candidate.size.map(rd_core::ByteCount::get),
            provider: candidate.provider,
            error: candidate.error,
        }
    }
}

/// A collector package together with its candidate links.
#[derive(Serialize)]
pub(crate) struct CollectorPackageItem {
    pub id: String,
    pub batch_id: String,
    pub name: String,
    pub category_id: Option<String>,
    pub priority: rd_core::DownloadPriority,
    pub has_password: bool,
    pub candidates: Vec<CandidateItem>,
}

impl CollectorPackageItem {
    pub(crate) fn new(package: rd_core::CollectorPackage, candidates: Vec<CandidateItem>) -> Self {
        Self {
            id: package.id.to_string(),
            batch_id: package.batch_id.to_string(),
            name: package.name,
            category_id: package.category_id.map(|id| id.to_string()),
            priority: package.priority,
            has_password: package.has_password,
            candidates,
        }
    }
}

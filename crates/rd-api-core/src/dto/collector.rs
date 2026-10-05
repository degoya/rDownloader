//! The LinkGrabber: intake, its packages and links, NZB imports and the hand-over to the queue.

use super::*;

/// Text or URL intake for the LinkGrabber.
#[derive(Deserialize, ToSchema)]
pub struct CollectorIntakeRequest {
    /// Free text the server scans for links; optional when `links` is used.
    #[serde(default)]
    pub text: Option<String>,
    pub source: rd_core::IngressSource,
    pub source_label: Option<String>,
    /// Explicit package name (Click'n'Load package or manual input); keeps all links together.
    pub package_name: Option<String>,
    /// Archive password announced with the links.
    ///
    /// Readable again on the package (RD-104-04); see `DownloadPackage::password`.
    pub password: Option<String>,
    /// Structured links with per-link request metadata (capture contract v1).
    #[serde(default)]
    pub links: Vec<CaptureLinkRequest>,
}

/// One structured link of a capture batch, optionally with the request that produced it.
#[derive(Deserialize, ToSchema)]
pub struct CaptureLinkRequest {
    #[schema(format = "uri")]
    pub url: String,
    pub file_name: Option<String>,
    /// Metadata to reproduce the GET; sanitized and allowlisted on arrival.
    #[serde(default)]
    pub request: Option<rd_core::CapturedRequest>,
}

/// Result of one intake: the batch, its packages and links.
#[derive(Serialize, ToSchema)]
pub struct CollectorIntakeResponse {
    pub batch: rd_core::CollectorBatch,
    pub packages: Vec<rd_core::CollectorPackage>,
    pub candidates: Vec<rd_core::LinkCandidate>,
    /// Links dropped by the domain blocklist before candidates were created.
    pub skipped_excluded: u32,
    /// Links dropped because the service that would carry them is switched off.
    pub skipped_disabled: u32,
    /// Addresses the folder crawlers and site rules handed back for this intake, before any
    /// of them was judged. Zero when no crawler ran.
    pub crawled_found: u32,
    /// How many of those were refused because nothing claims them and no probe confirmed
    /// them to be files (RD-110-07). A rule reaching one element too far shows up here
    /// rather than as a page that quietly became a download.
    pub crawled_dropped: u32,
}

/// Editable LinkGrabber package fields.
#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageUpdateRequest {
    pub name: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
    /// Archive password; readable again on the package (RD-104-04).
    pub password: Option<String>,
    #[serde(default)]
    pub clear_password: bool,
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub clear_postprocess_level: bool,
    pub script: Option<String>,
    #[serde(default)]
    pub clear_script: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageBulkRequest {
    pub ids: Vec<rd_core::CollectorPackageId>,
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

/// Editable routing metadata of an NZB waiting in the LinkGrabber.
#[derive(Deserialize, ToSchema)]
pub struct NzbImportUpdateRequest {
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
}

/// How an NZB import enters the download queue.
///
/// The body is optional: a request without one keeps the previous behaviour and starts the
/// package immediately.
#[derive(Default, Deserialize, ToSchema)]
pub struct NzbImportEnqueueRequest {
    /// Creates every download of the package paused instead of starting it immediately.
    #[serde(default)]
    pub paused: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageReorderRequest {
    pub ids: Vec<rd_core::CollectorPackageId>,
}

/// A slice of the LinkGrabber's manual order over both kinds of entry, in display order.
///
/// A list may be partial - the client sends the rows it is showing, and filters hide rows - but
/// every entry has to name a row that exists under the kind it claims.
#[derive(Deserialize, ToSchema)]
pub struct GrabberEntryReorderRequest {
    pub entries: Vec<rd_core::GrabberEntryRef>,
    /// The entry the listed ones are placed behind; absent means the head of the list.
    ///
    /// Without it a partial list can only describe a prefix, so a drag deep into a long list has
    /// to send everything above it and runs into the bulk bound. The anchor is what keeps a drag
    /// at index 700 the same two entries as a drag at index 2.
    #[serde(default)]
    pub after: Option<rd_core::GrabberEntryRef>,
}

/// Packages to enqueue in the given (displayed) order.
#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageEnqueueRequest {
    pub ids: Vec<rd_core::CollectorPackageId>,
    /// Enqueues only these links of the packages; the others stay in the LinkGrabber, in their
    /// package. What the LinkGrabber sends while a filter hides part of a package — absent, a
    /// package goes whole.
    #[serde(default)]
    pub candidate_ids: Option<Vec<rd_core::CandidateId>>,
    /// Creates every download in paused state instead of starting it immediately.
    #[serde(default)]
    pub paused: bool,
}

/// Result of a batch enqueue; partial failures are reported instead of silently dropped.
#[derive(Serialize, ToSchema)]
pub struct CollectorEnqueueBatchResponse {
    pub created: Vec<rd_core::DownloadPackage>,
    /// Packages that could not be enqueued.
    pub failed: u32,
    /// Message of the first failure, when any package failed.
    pub first_error: Option<String>,
    /// Files enqueued without a provider account (free/direct download attempt).
    pub free_download_files: u32,
}

#[derive(Deserialize, ToSchema)]
pub struct CandidateMoveRequest {
    pub ids: Vec<rd_core::CandidateId>,
    pub package_id: Option<rd_core::CollectorPackageId>,
    pub new_package_name: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CandidateReorderRequest {
    pub package_id: rd_core::CollectorPackageId,
    pub ids: Vec<rd_core::CandidateId>,
}

/// Links to check; empty = every open link.
#[derive(Deserialize, ToSchema)]
pub struct CandidateCheckRequest {
    pub ids: Option<Vec<rd_core::CandidateId>>,
}

#[derive(Deserialize, ToSchema)]
pub struct CandidateRenameRequest {
    pub file_name: Option<String>,
    /// Switches the media variant (`best`, `1080p`, `audio_mp3`, …) of a media link.
    pub media_variant: Option<String>,
}

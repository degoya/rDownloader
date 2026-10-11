//! Exporting packages as a link file, and re-resolving downloads with the plugin installed now
//! (RD-1210-01), at once if asked (RD-1250-05).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::links_file::Passphrase;

/// The file an export writes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageExportFormat {
    /// rDownloader's own `.rdlinks`: every package field and link detail, optionally sealed.
    Rdlinks,
    /// JDownloader's `.crawljob`: addresses, package name and password, never sealed.
    Crawljob,
}

/// What to export: any mix of download-list packages, single downloads and LinkGrabber
/// packages, or every package of the download list.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageExportRequest {
    /// Download-list packages, with every file they hold.
    #[serde(default)]
    pub package_ids: Vec<rd_core::PackageId>,
    /// Single downloads; each lands in its own package's entry.
    #[serde(default)]
    pub download_ids: Vec<rd_core::DownloadId>,
    /// LinkGrabber packages, with every link not yet queued.
    #[serde(default)]
    pub collector_package_ids: Vec<rd_core::CollectorPackageId>,
    /// Every package of the download list, finished, running and failed alike.
    #[serde(default)]
    pub all: bool,
    pub format: PackageExportFormat,
    /// Seals an `.rdlinks` file; at least 8 characters. Never logged, audited or answered back.
    #[serde(default)]
    #[schema(value_type = Option<String>, write_only)]
    pub passphrase: Option<Passphrase>,
}

/// Downloads to resolve again with the plugin installed now: any mix of single downloads and
/// whole packages.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReresolveRequest {
    #[serde(default)]
    pub ids: Vec<rd_core::DownloadId>,
    #[serde(default)]
    pub package_ids: Vec<rd_core::PackageId>,
    /// Resolve the files now, without downloading them, and write back the size, the name and
    /// any failure (RD-1250-05). Off: they resolve at their next start, as before.
    #[serde(default)]
    pub check_now: bool,
}

/// The answer to a re-resolve: the bulk answer's counts and refusals and, with `check_now`,
/// what resolving each file now found.
#[derive(Serialize, ToSchema)]
pub struct ReresolveResponse {
    /// Files that resolve anew with the plugin installed now.
    pub affected: u32,
    pub errors: Vec<String>,
    /// The same refusals as `errors`, coded and in the same order.
    pub refusals: Vec<crate::dto::MessageResponse>,
    /// With `check_now`: files that resolved, online or held in a provider's cache.
    pub resolved: u32,
    /// With `check_now`: files whose resolution failed or found them offline.
    pub failed: u32,
    /// With `check_now`: hoster links without an account, which are not resolved ahead because
    /// that would run the hoster's free flow (owner, 2026-10-11); they resolve at their start.
    pub skipped: u32,
    /// With `check_now`: one entry per file that was resolved now, in request order. Files of
    /// a kind with no resolver step (Usenet, torrents, media, remote folders) are not listed.
    pub results: Vec<ReresolveResult>,
}

/// What resolving one file now found.
#[derive(Serialize, ToSchema)]
pub struct ReresolveResult {
    pub download_id: rd_core::DownloadId,
    pub status: rd_core::LinkStatus,
    /// The name the resolver or the server declared, as written to the file.
    pub file_name: Option<String>,
    /// The size the resolver or the server reported.
    pub total_bytes: Option<rd_core::ByteCount>,
    /// Not resolved ahead: a hoster link without an account (`download.resolve_skipped_free`
    /// in `failure`). Nothing was asked of the hoster and nothing written.
    pub skipped: bool,
    /// Why the file did not resolve, coded like a download's last error.
    pub failure: Option<rd_core::Failure>,
}

//! Exporting packages as a link file, and re-resolving downloads with the plugin installed now
//! (RD-1210-01).

use serde::Deserialize;
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
}

//! The commands of `writer/packages.rs`.

use super::Reply;

/// The commands `Writer::handle_packages` applies.
pub(crate) enum PackagesCommand {
    /// Carries those fields onto the package and files an enqueue created (RD-107-02).
    CarryEnrichment {
        package_id: rd_core::PackageId,
        /// The union across the package's files, shown on the package header.
        package_fields: Vec<rd_core::EnrichmentField>,
        /// Per queue row, so a package of several releases keeps them apart.
        files: Vec<(rd_core::DownloadId, Vec<rd_core::EnrichmentField>)>,
        reply: Reply<()>,
    },
    UpdatePackages {
        ids: Vec<rd_core::PackageId>,
        change: crate::package_store::PackageChange,
        reply: Reply<Vec<rd_core::DownloadPackage>>,
    },
    RenamePackageDirectory {
        id: rd_core::PackageId,
        name: String,
        destination: String,
        reply: Reply<Option<rd_core::DownloadPackage>>,
    },
    ClearPreviousDestination {
        id: rd_core::PackageId,
        reply: Reply<()>,
    },
    /// The commit of a torrent move (RD-1100-10); `false` when the package no longer names
    /// `from`.
    SwitchPackageDestination {
        id: rd_core::PackageId,
        from: String,
        to: String,
        reply: Reply<bool>,
    },
    ReorderPackages {
        ids: Vec<rd_core::PackageId>,
        reply: Reply<()>,
    },
    ReorderDownloads {
        package_id: rd_core::PackageId,
        ids: Vec<rd_core::DownloadId>,
        reply: Reply<()>,
    },
    SetPackageState {
        id: rd_core::PackageId,
        state: rd_core::PackageState,
        stage: Option<rd_core::PostprocessStage>,
        percent: Option<u8>,
        current: Option<String>,
        reply: Reply<()>,
    },
    SetPackageExtraction {
        id: rd_core::PackageId,
        result: Option<rd_core::ExtractionResult>,
        reply: Reply<()>,
    },
    /// Sets or removes (`None`) the package's own download limit (RD-1100-01).
    SetPackageSpeedLimit {
        id: rd_core::PackageId,
        bytes_per_second: Option<u64>,
        reply: Reply<()>,
    },
    /// Sets the queue's stop mark, replacing the one in force (RD-1210-02).
    SetStopMark {
        target: crate::StopMarkTarget,
        reply: Reply<crate::StopMark>,
    },
    /// Removes the stop mark; with `only`, only while it sits on that target (RD-1210-02).
    ClearStopMark {
        only: Option<crate::StopMarkTarget>,
        reply: Reply<bool>,
    },
}

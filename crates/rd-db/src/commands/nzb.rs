//! The commands of `writer/nzb.rs`.

use rd_core::DownloadId;

use super::Reply;
use crate::nzb_store::NewNzbImport;

/// The commands `Writer::handle_nzb` applies.
pub(crate) enum NzbCommand {
    /// The assembled file of a Usenet row is on disk: final name, PAR2 marking decided again
    /// on that name and the content, and the set's waiting volumes postponed if this is the
    /// main index (RD-108-23). Replies with the number of volumes postponed.
    SettleNzbRecovery {
        id: DownloadId,
        file_name: String,
        /// The file starts with the PAR2 packet magic.
        content_is_par2: bool,
        reply: Reply<usize>,
    },
    /// A Usenet file is assembled but has holes where articles were missing, and whether the
    /// set can repair them is not yet known (RD-108-24). Holds the verdict open on the row
    /// until the package settles.
    DeferPar2Verdict {
        id: DownloadId,
        /// Segments no server had.
        missing: usize,
        reply: Reply<()>,
    },
    AddNzbImport {
        import: NewNzbImport,
        reply: Reply<rd_core::NzbImport>,
    },
    RecordNzbImportFailure {
        failure: crate::nzb_store::FailedNzbImport,
        reply: Reply<rd_core::NzbImport>,
    },
    UpdateNzbImport {
        id: rd_core::NzbImportId,
        change: crate::nzb_store::NzbImportChange,
        reply: Reply<rd_core::NzbImport>,
    },
    DeleteNzbImport {
        id: rd_core::NzbImportId,
        reply: Reply<()>,
    },
    MarkNzbImportRemoteJob {
        id: rd_core::NzbImportId,
        remote_job_id: rd_core::RemoteJobId,
        expected: rd_core::NzbImportState,
        reply: Reply<rd_core::NzbImport>,
    },
    ForgetNzbImportHistory {
        package_id: rd_core::PackageId,
        reply: Reply<()>,
    },
    SetNzbSegmentState {
        id: rd_core::NzbSegmentId,
        state: rd_core::NzbSegmentState,
        crc32: Option<u32>,
        reply: Reply<()>,
    },
    CheckpointNzb {
        checkpoint: crate::postprocess_store::NzbCheckpoint,
        reply: Reply<()>,
    },
    EnqueueNzbImport {
        id: rd_core::NzbImportId,
        destination: std::path::PathBuf,
        priority: rd_core::DownloadPriority,
        /// Creates every download row of the package paused instead of queued.
        start_paused: bool,
        reply: Reply<rd_core::PackageId>,
    },
    /// A Usenet set turned out to be beyond repair (RD-1100-02): its waiting rows and the rows
    /// holding a PAR2 verdict fail with `failure`, and the package is settled again.
    FailHopelessPackage {
        package_id: rd_core::PackageId,
        failure: rd_core::Failure,
        reply: Reply<()>,
    },
}

//! The commands of `writer/downloads.rs`.

use chrono::{DateTime, Utc};
use rd_core::{ChunkId, DownloadFile, DownloadId, DownloadState};

use super::Reply;
use crate::models::{NewDownload, NewPackage, PersistedChunk};

/// The commands `Writer::handle_downloads` applies.
// `CreateDownload` outweighs the rest of the area; every message is as large as the largest
// command, as it was with one flat enum.
#[allow(clippy::large_enum_variant)]
pub(crate) enum DownloadsCommand {
    CreatePackage {
        package: NewPackage,
        reply: Reply<rd_core::DownloadPackage>,
    },
    CreateDownload {
        download: NewDownload,
        /// Every source of the file and its hashes (RD-150-03), written in the same
        /// transaction as the row.
        sources: Option<Box<rd_core::SourceSet>>,
        reply: Reply<DownloadFile>,
    },
    /// One event for the rows one enqueue created (RD-1120-17).
    AnnounceCreated {
        package_id: rd_core::PackageId,
        ids: Vec<DownloadId>,
        reply: Reply<()>,
    },
    TransitionDownload {
        id: DownloadId,
        next: DownloadState,
        reply: Reply<DownloadFile>,
    },
    /// Queues a row its enqueue wrote paused, unless somebody touched it since `created_at`.
    JoinQueue {
        id: DownloadId,
        created_at: DateTime<Utc>,
        reply: Reply<DownloadFile>,
    },
    /// Blocks a download and records why, so a release path can tell the causes apart.
    BlockDownload {
        id: DownloadId,
        /// Stable identifier of the cause; the caller owns the vocabulary.
        reason: String,
        reply: Reply<DownloadFile>,
    },
    DeleteDownload {
        id: DownloadId,
        reply: Reply<()>,
    },
    /// Removes many inactive rows in one transaction (RD-1120-17); one answer per id, in order.
    DeleteDownloads {
        ids: Vec<DownloadId>,
        reply: Reply<Vec<anyhow::Result<()>>>,
    },
    /// Removes a package that has no files, for a caller with no download id to offer.
    DeleteEmptyPackage {
        id: rd_core::PackageId,
        reply: Reply<bool>,
    },
    CheckpointChunk {
        chunk_id: ChunkId,
        committed_offset: u64,
        reply: Reply<()>,
    },
    /// Records one finished provider-chunk MAC of a transformed stream (RD-103-02).
    CheckpointChunkMac {
        download_id: DownloadId,
        /// `ContentTransform::fingerprint` of the description that produced it. A row
        /// written by any other description is dropped rather than mixed in.
        fingerprint: String,
        index: u64,
        mac: [u8; 16],
        reply: Reply<()>,
    },
    /// Progress of a runner-driven file (no chunk rows), e.g. media downloads.
    SetDownloadProgress {
        id: DownloadId,
        committed_bytes: u64,
        total_bytes: Option<u64>,
        reply: Reply<()>,
    },
    /// Replaces the torrent state blob of one link candidate.
    SetCandidateTorrentState {
        id: rd_core::CandidateId,
        /// Boxed: the file tree makes this by far the largest command variant.
        state: Box<rd_core::TorrentCandidateState>,
        reply: Reply<()>,
    },
    /// Replaces the torrent state blob of one queue row.
    SetDownloadTorrentState {
        id: DownloadId,
        state: Box<rd_core::TorrentJobState>,
        reply: Reply<()>,
    },
    PrepareTransfer {
        id: DownloadId,
        total_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        chunks: Vec<PersistedChunk>,
        reply: Reply<()>,
    },
    RecordFailure {
        id: DownloadId,
        failure: rd_core::Failure,
        retry_at: Option<DateTime<Utc>>,
        reply: Reply<DownloadFile>,
    },
    /// When the automatic retry takes a failed download up again (RD-191-12).
    ScheduleAutoRetry {
        id: DownloadId,
        at: Option<DateTime<Utc>>,
        reply: Reply<bool>,
    },
    /// A failed download back to `queued` with a fresh retry budget: a round of the automatic
    /// retry (counted) or a person's resume.
    RequeueFailed {
        id: DownloadId,
        auto_retry: bool,
        reply: Reply<Option<DownloadFile>>,
    },
    CompleteDownload {
        id: DownloadId,
        final_name: String,
        checksum: Option<rd_core::ExpectedChecksum>,
        reply: Reply<DownloadFile>,
    },
    SetFileName {
        id: DownloadId,
        file_name: String,
        reply: Reply<()>,
    },
    /// The vault reference of this download's transform key (RD-120-11).
    ///
    /// `None` clears it, which is what a download whose transform went away needs; the vault
    /// entry itself is removed by the caller, because the writer owns rows and not files.
    SetTransformKeyRef {
        id: DownloadId,
        reference: Option<String>,
        reply: Reply<()>,
    },
    RenameDownload {
        id: DownloadId,
        file_name: String,
        reply: Reply<DownloadFile>,
    },
    ClaimResolverRefresh {
        id: DownloadId,
        reply: Reply<bool>,
    },
    ClaimReplayRefresh {
        id: DownloadId,
        reply: Reply<bool>,
    },
    ResetDownload {
        id: DownloadId,
        reply: Reply<rd_core::DownloadFile>,
    },
    ResetTransfer {
        id: DownloadId,
        reply: Reply<()>,
    },
    SetCandidateReplayConsent {
        id: rd_core::CandidateId,
        consent: Box<Option<rd_core::ReplayConsent>>,
        reply: Reply<()>,
    },
    ClaimResolverPin {
        id: DownloadId,
        pin: rd_core::ResolverPin,
        reply: Reply<rd_core::ResolverPin>,
    },
    ClearUnsatisfiableResolverPins {
        /// `(plugin_id, version)` of every resolver this build can actually provide.
        available: Vec<(String, String)>,
        reply: Reply<u64>,
    },
    /// Points a download that is not running at one exact resolver version (RD-140-02).
    PinDownloadResolver {
        id: DownloadId,
        pin: rd_core::ResolverPin,
        reply: Reply<()>,
    },
}

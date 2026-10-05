//! Database facade for download sources, piece hashes and chunk marks (RD-150-03).

use anyhow::Result;
use rd_core::{
    CandidateId, ChunkId, DownloadFile, DownloadId, DownloadSource, PieceHashes, SourceOutcome,
    SourceSet,
};

use crate::{
    ChunkMark, Database,
    commands::{DownloadsCommand, SourcesCommand},
    download_sources_store,
    models::NewDownload,
    writer,
};

impl Database {
    /// Creates a download together with every source of its file and the piece hashes.
    ///
    /// One transaction: a transfer that could start between the row and its sources would
    /// fetch the whole file from one mirror.
    pub async fn create_download_with_sources(
        &self,
        download: NewDownload,
        sources: SourceSet,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::CreateDownload {
            download,
            sources: Some(Box::new(sources)),
            reply,
        })
        .await
    }

    /// Every source of a download in its fixed order; empty for a download without a set.
    pub async fn download_sources(&self, id: DownloadId) -> Result<Vec<DownloadSource>> {
        download_sources_store::list_sources(&self.readers, id).await
    }

    /// The piece hashes a download's set stated, if any.
    pub async fn download_piece_hashes(&self, id: DownloadId) -> Result<Option<PieceHashes>> {
        download_sources_store::piece_hashes(&self.readers, id).await
    }

    /// Records what an attempt learned about one source.
    pub async fn record_source_outcome(
        &self,
        download_id: DownloadId,
        position: u32,
        outcome: SourceOutcome,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| SourcesCommand::RecordSourceOutcome {
            download_id,
            position,
            outcome,
            reply,
        })
        .await
    }

    /// The source and verification marks of every chunk of a download.
    pub async fn chunk_marks(&self, id: DownloadId) -> Result<Vec<ChunkMark>> {
        download_sources_store::chunk_marks(&self.readers, id).await
    }

    /// Notes which source a chunk's bytes came from and whether its pieces were checked.
    pub async fn mark_chunk(
        &self,
        chunk_id: ChunkId,
        source_position: Option<u32>,
        verified: bool,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| SourcesCommand::MarkChunk {
            chunk_id,
            source_position,
            verified,
            reply,
        })
        .await
    }

    /// Moves a chunk's confirmed offset back over bytes a piece hash refused.
    pub async fn rewind_chunk(&self, chunk_id: ChunkId, committed: u64) -> Result<()> {
        writer::request(&self.writer, |reply| SourcesCommand::RewindChunk {
            chunk_id,
            committed,
            reply,
        })
        .await
    }

    /// Keeps a checked source set on a LinkGrabber candidate until it is queued.
    pub async fn set_candidate_source_set(
        &self,
        candidate_id: CandidateId,
        set: SourceSet,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            SourcesCommand::SetCandidateSourceSet {
                candidate_id,
                set: Box::new(set),
                reply,
            }
        })
        .await
    }

    /// Holds candidates a stranger's document proposed — a Metalink's link, a parser's or a
    /// crawler's find — to an address reach (RD-150-03); the online check keeps to it.
    pub async fn set_candidates_remote_reach(
        &self,
        candidate_ids: Vec<CandidateId>,
        local_network: bool,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            SourcesCommand::SetCandidateRemoteReach {
                candidate_ids,
                local_network,
                reply,
            }
        })
        .await
    }

    /// Every candidate held to an address reach, with whether it may reach the person's own
    /// network; a candidate the person added themselves is absent.
    pub async fn candidates_remote_reach(
        &self,
    ) -> Result<std::collections::HashMap<CandidateId, bool>> {
        download_sources_store::candidates_remote_reach(&self.readers).await
    }

    /// The address reach one candidate is held to (RD-150-03): `Some(local_network)` for a
    /// link a stranger's document or page proposed, `None` for one the person added.
    pub async fn candidate_remote_reach(&self, candidate_id: CandidateId) -> Result<Option<bool>> {
        download_sources_store::candidate_remote_reach(&self.readers, candidate_id).await
    }

    /// The source set a LinkGrabber candidate carries, if any.
    pub async fn candidate_source_set(
        &self,
        candidate_id: CandidateId,
    ) -> Result<Option<SourceSet>> {
        download_sources_store::candidate_source_set(&self.readers, candidate_id).await
    }
}

//! The commands of `writer/sources.rs`.

use rd_core::{ChunkId, DownloadId};

use super::Reply;

/// The commands `Writer::handle_sources` applies.
pub(crate) enum SourcesCommand {
    /// What an attempt learned about one source of a download (RD-150-03).
    RecordSourceOutcome {
        download_id: DownloadId,
        position: u32,
        outcome: rd_core::SourceOutcome,
        reply: Reply<()>,
    },
    /// Which source delivered a chunk and whether its pieces were checked.
    MarkChunk {
        chunk_id: ChunkId,
        source_position: Option<u32>,
        verified: bool,
        reply: Reply<()>,
    },
    /// Moves a chunk's confirmed offset back over bytes a piece hash refused.
    RewindChunk {
        chunk_id: ChunkId,
        committed: u64,
        reply: Reply<()>,
    },
    /// Keeps a checked source set on a LinkGrabber candidate until it is queued.
    SetCandidateSourceSet {
        candidate_id: rd_core::CandidateId,
        set: Box<rd_core::SourceSet>,
        reply: Reply<()>,
    },
    /// Holds LinkGrabber candidates a stranger's document proposed to an address reach.
    SetCandidateRemoteReach {
        candidate_ids: Vec<rd_core::CandidateId>,
        local_network: bool,
        reply: Reply<()>,
    },
}

//! The commands of `writer/storage.rs`.

use rd_core::DownloadId;

use super::Reply;

/// The commands `Writer::handle_storage` applies.
pub(crate) enum StorageCommand {
    /// Sets or clears the collision policy of a category or a package (RD-150-01).
    SetCollisionPolicy {
        scope_kind: &'static str,
        scope_id: String,
        policy: Option<rd_core::CollisionPolicy>,
        reply: Reply<()>,
    },
    OpenCollisionPrompt {
        prompt: crate::NewCollisionPrompt,
        reply: Reply<()>,
    },
    DecideCollisionPrompt {
        download_id: DownloadId,
        decision: rd_core::CollisionDecision,
        reply: Reply<bool>,
    },
    ClearCollisionPrompt {
        download_id: DownloadId,
        reply: Reply<()>,
    },
    IndexContent {
        download_id: DownloadId,
        algorithm: String,
        digest: String,
        size_bytes: u64,
        path: String,
        reply: Reply<()>,
    },
    MoveIndexedContent {
        download_id: DownloadId,
        path: String,
        reply: Reply<()>,
    },
    MarkIndexedContent {
        changes: Vec<(DownloadId, bool)>,
        reply: Reply<()>,
    },
    ForgetIndexedPath {
        path: String,
        except: DownloadId,
        reply: Reply<u64>,
    },
    /// Empties the content index (RD-180-13).
    ClearContentIndex {
        reply: Reply<u64>,
    },
    /// Records the start of a verified move or a dedupe link (RD-150-02).
    StartStorageOperation {
        operation: crate::NewStorageOperation,
        reply: Reply<i64>,
    },
    FinishStorageOperation {
        id: i64,
        outcome: crate::StorageOperationOutcome,
        reply: Reply<()>,
    },
    InterruptStorageOperations {
        reply: Reply<u64>,
    },
    /// Empties the storage history except the rows still running (RD-180-13).
    ClearStorageOperations {
        reply: Reply<u64>,
    },
}

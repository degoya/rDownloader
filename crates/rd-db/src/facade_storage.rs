//! Database facade methods for collision policies, prompts, the content index and the storage
//! operation history (RD-150-01, RD-150-02).

use anyhow::Result;
use rd_core::{CollisionDecision, CollisionPolicy, DownloadId, PackageId};

use crate::{
    Database,
    collision_store::{
        self, CollisionPolicyLevels, CollisionPolicyRow, CollisionPrompt, ContentIndexEntry,
        NewCollisionPrompt,
    },
    commands::WriterCommand,
    storage_ops_store::{self, NewStorageOperation, StorageOperation, StorageOperationOutcome},
    writer,
};

impl Database {
    /// Every policy a category or a package holds of its own.
    pub async fn list_collision_policies(&self) -> Result<Vec<CollisionPolicyRow>> {
        collision_store::list_collision_policies(&self.readers).await
    }

    /// The package's own policy and its category's; the global one is in the settings.
    pub async fn collision_policy_levels(
        &self,
        package_id: PackageId,
    ) -> Result<CollisionPolicyLevels> {
        collision_store::collision_policy_levels(&self.readers, package_id).await
    }

    /// Sets a category's policy, or clears it with `None` so the category inherits.
    pub async fn set_category_collision_policy(
        &self,
        category_id: rd_core::CategoryId,
        policy: Option<CollisionPolicy>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetCollisionPolicy {
            scope_kind: collision_store::SCOPE_CATEGORY,
            scope_id: category_id.to_string(),
            policy,
            reply,
        })
        .await
    }

    /// Sets a package's policy, or clears it with `None` so the package inherits.
    pub async fn set_package_collision_policy(
        &self,
        package_id: PackageId,
        policy: Option<CollisionPolicy>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetCollisionPolicy {
            scope_kind: collision_store::SCOPE_PACKAGE,
            scope_id: package_id.to_string(),
            policy,
            reply,
        })
        .await
    }

    pub async fn collision_prompt(
        &self,
        download_id: DownloadId,
    ) -> Result<Option<CollisionPrompt>> {
        collision_store::collision_prompt(&self.readers, download_id).await
    }

    pub async fn list_collision_prompts(&self) -> Result<Vec<CollisionPrompt>> {
        collision_store::list_collision_prompts(&self.readers).await
    }

    /// Opens the prompt of a download whose file collided under `ask`.
    pub async fn open_collision_prompt(&self, prompt: NewCollisionPrompt) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::OpenCollisionPrompt {
            prompt,
            reply,
        })
        .await
    }

    /// Records the answer to a prompt; `false` when the download has none.
    pub async fn decide_collision_prompt(
        &self,
        download_id: DownloadId,
        decision: CollisionDecision,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::DecideCollisionPrompt {
            download_id,
            decision,
            reply,
        })
        .await
    }

    /// Removes the prompt once its answer has been carried out.
    pub async fn clear_collision_prompt(&self, download_id: DownloadId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ClearCollisionPrompt {
            download_id,
            reply,
        })
        .await
    }

    pub async fn content_index_entry(
        &self,
        download_id: DownloadId,
    ) -> Result<Option<ContentIndexEntry>> {
        collision_store::content_index_entry(&self.readers, download_id).await
    }

    /// Every indexed file with this digest, missing ones included.
    pub async fn content_index_matches(
        &self,
        algorithm: &str,
        digest: &str,
    ) -> Result<Vec<ContentIndexEntry>> {
        collision_store::content_index_matches(&self.readers, algorithm, digest).await
    }

    pub async fn list_content_index(&self) -> Result<Vec<ContentIndexEntry>> {
        collision_store::list_content_index(&self.readers).await
    }

    /// Records where a finished file lies and what it hashes to.
    pub async fn index_content(
        &self,
        download_id: DownloadId,
        algorithm: String,
        digest: String,
        size_bytes: u64,
        path: String,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::IndexContent {
            download_id,
            algorithm,
            digest,
            size_bytes,
            path,
            reply,
        })
        .await
    }

    /// Follows a moved file with its index entry.
    pub async fn move_indexed_content(&self, download_id: DownloadId, path: String) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::MoveIndexedContent {
            download_id,
            path,
            reply,
        })
        .await
    }

    /// Marks entries missing (`true`) or present again (`false`).
    pub async fn mark_indexed_content(&self, changes: Vec<(DownloadId, bool)>) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        writer::request(&self.writer, |reply| WriterCommand::MarkIndexedContent {
            changes,
            reply,
        })
        .await
    }

    /// Forgets the other downloads' entries for `path` after `except`'s file replaced it;
    /// answers how many there were.
    pub async fn forget_indexed_path(&self, path: String, except: DownloadId) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::ForgetIndexedPath {
            path,
            except,
            reply,
        })
        .await
    }

    /// Entries in the content index, missing ones included.
    pub async fn count_content_index(&self) -> Result<u64> {
        collision_store::count_content_index(&self.readers).await
    }

    /// Empties the content index; answers how many entries went (RD-180-13). Files and
    /// downloads stay; the next check backfills what it can find through the rows.
    pub async fn clear_content_index(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::ClearContentIndex {
            reply,
        })
        .await
    }

    /// History rows a clear would remove: every one not still running.
    pub async fn count_clearable_storage_operations(&self) -> Result<u64> {
        storage_ops_store::count_clearable_storage_operations(&self.readers).await
    }

    /// Empties the storage history except the rows still running; answers how many went
    /// (RD-180-13).
    pub async fn clear_storage_operations(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ClearStorageOperations { reply }
        })
        .await
    }

    /// Newest first, at most `limit`.
    pub async fn list_storage_operations(&self, limit: u32) -> Result<Vec<StorageOperation>> {
        storage_ops_store::list_storage_operations(&self.readers, limit).await
    }

    pub async fn start_storage_operation(&self, operation: NewStorageOperation) -> Result<i64> {
        writer::request(&self.writer, |reply| WriterCommand::StartStorageOperation {
            operation,
            reply,
        })
        .await
    }

    pub async fn finish_storage_operation(
        &self,
        id: i64,
        outcome: StorageOperationOutcome,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::FinishStorageOperation { id, outcome, reply }
        })
        .await
    }

    /// Settles the operations a stopped process left running; answers how many there were.
    pub async fn interrupt_storage_operations(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            WriterCommand::InterruptStorageOperations { reply }
        })
        .await
    }

    /// Packages whose category change still has data to carry over.
    pub async fn packages_with_outstanding_move(&self) -> Result<Vec<PackageId>> {
        storage_ops_store::packages_with_outstanding_move(&self.readers).await
    }
}

//! Database facade for plugin bookkeeping: invocations, transfer checkpoints, the versions
//! downloads are bound to, remote jobs, managed tools, signing keys, withdrawn digests and
//! version choices.

use anyhow::Result;
use rd_core::DownloadId;

use crate::{
    Database, ManagedToolRecord, NewManagedTool, NewPluginDigestRevocation, NewPluginExecution,
    NewPluginTrustedKey, NewPluginVersionChoice, PluginDigestRevocation, PluginExecution,
    PluginTransfer, PluginTrustedKey, PluginVersionChoice, ToolManifestState,
    commands::PluginsCommand, managed_tools_store, plugin_execution_store, plugin_keys_store,
    plugin_revocations_store, plugin_transfer_store, plugin_versions_store, writer,
};

/// Every unfinished download bound to one plugin id and version, by download id.
///
/// The two records that name a version: the resolver pin a job claimed, and the checkpoint a
/// transfer backend wrote. Takes the id and the version twice, once per branch.
const PLUGIN_VERSION_BINDINGS: &str = "\
    SELECT pin.download_id AS download_id FROM download_resolver_pins pin \
      JOIN downloads job ON job.id = pin.download_id \
     WHERE pin.plugin_id = ? AND pin.plugin_version = ? AND job.state != 'completed' \
    UNION \
    SELECT transfer.download_id AS download_id FROM plugin_transfers transfer \
      JOIN downloads job ON job.id = transfer.download_id \
     WHERE transfer.plugin_id = ? AND transfer.plugin_version = ? AND job.state != 'completed'";

impl Database {
    /// The newest recorded invocations of one plugin, newest first.
    pub async fn plugin_executions(
        &self,
        plugin_id: &str,
        limit: i64,
    ) -> Result<Vec<PluginExecution>> {
        plugin_execution_store::list_plugin_executions(&self.readers, plugin_id, limit).await
    }

    /// How many recorded invocations each plugin has, for the plugins that have any.
    ///
    /// One grouped read, so the plugin manager can tell a plugin that never ran from one that
    /// ran without incident without fetching a single entry.
    pub async fn plugin_execution_counts(&self) -> Result<Vec<(String, i64)>> {
        plugin_execution_store::plugin_execution_counts(&self.readers).await
    }

    /// Records one invocation and trims the plugin's history to its cap.
    ///
    /// Diagnostics are optional by definition: a caller that cannot write one carries on with
    /// the download rather than failing it.
    pub async fn record_plugin_execution(&self, entry: NewPluginExecution) -> Result<()> {
        writer::request(&self.writer, |reply| {
            PluginsCommand::RecordPluginExecution {
                entry: Box::new(entry),
                reply,
            }
        })
        .await
    }

    /// Resume state of a plugin transfer, if the job has one.
    pub async fn plugin_transfer(&self, id: DownloadId) -> Result<Option<PluginTransfer>> {
        plugin_transfer_store::load_plugin_transfer(&self.readers, id).await
    }

    /// Persists a transfer backend's checkpoint, claiming the version pin on first write.
    pub async fn save_plugin_transfer(
        &self,
        id: DownloadId,
        plugin_id: String,
        plugin_version: String,
        checkpoint: Option<Vec<u8>>,
    ) -> Result<PluginTransfer> {
        writer::request(&self.writer, |reply| PluginsCommand::SavePluginTransfer {
            id,
            plugin_id,
            plugin_version,
            checkpoint,
            reply,
        })
        .await
    }

    /// Forgets a transfer's resume state once it finished or was discarded.
    pub async fn clear_plugin_transfer(&self, id: DownloadId) -> Result<()> {
        writer::request(&self.writer, |reply| PluginsCommand::ClearPluginTransfer {
            id,
            reply,
        })
        .await
    }

    /// How many unfinished downloads are bound to exactly this plugin id and version.
    ///
    /// Bound means a record decides how the job continues, and names that one version: the
    /// resolver pin a job claimed, or the checkpoint a transfer backend wrote. Both are
    /// durable on purpose -- a paused job resumes with the version that started it -- which is
    /// why the in-memory leases `rd-tools` keeps for external binaries do not transfer here.
    /// A restart forgets a lease; it must not forget that a paused job still needs one
    /// version out of two installed ones.
    ///
    /// Only a `completed` download is finished for this purpose. Everything else can still be
    /// started, resumed or retried by hand -- a cancelled job included: `ProgressControl::cancel`
    /// keeps the partial data and `resume` puts it back in the queue, so its checkpoint is a
    /// claim like any other. A job that is really gone is deleted, and deleting it takes its
    /// pin and its checkpoint with it.
    pub async fn plugin_version_usage(&self, plugin_id: &str, version: &str) -> Result<u64> {
        use sqlx::Row;

        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) AS bound FROM ({PLUGIN_VERSION_BINDINGS})"
        )))
        .bind(plugin_id)
        .bind(version)
        .bind(plugin_id)
        .bind(version)
        .fetch_one(&self.readers)
        .await?;
        Ok(u64::try_from(row.get::<i64, _>("bound")).unwrap_or(0))
    }

    /// Names the first `limit` downloads that hold this plugin version, oldest first.
    ///
    /// A refusal that only says how many jobs are in the way leaves the reader looking for
    /// them; the file names are what makes the blocker findable in the queue.
    pub async fn plugin_version_blockers(
        &self,
        plugin_id: &str,
        version: &str,
        limit: i64,
    ) -> Result<Vec<String>> {
        use sqlx::Row;

        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT job.file_name AS file_name FROM ({PLUGIN_VERSION_BINDINGS}) binding \
               JOIN downloads job ON job.id = binding.download_id \
              ORDER BY job.created_at LIMIT ?"
        )))
        .bind(plugin_id)
        .bind(version)
        .bind(plugin_id)
        .bind(version)
        .bind(limit)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.get::<String, _>("file_name"))
            .collect())
    }

    /// Writes the row that stands for one remote job, before the provider is asked for
    /// anything (RD-107-06).
    ///
    /// Deliberately the first write of the whole flow. The content key it carries is what
    /// makes a duplicate preventable at a provider whose submit is not idempotent: the unique
    /// index refuses a second row, so there is never a second row to drive a second submit.
    pub async fn claim_remote_job(
        &self,
        input: crate::remote_job_store::ClaimRemoteJob,
    ) -> Result<rd_core::RemoteJob> {
        writer::request(&self.writer, |reply| PluginsCommand::ClaimRemoteJob {
            input: Box::new(input),
            reply,
        })
        .await
    }

    /// Records what one submit, poll or answer changed about a remote job (RD-107-06).
    pub async fn advance_remote_job(
        &self,
        id: rd_core::RemoteJobId,
        input: crate::remote_job_store::AdvanceRemoteJob,
    ) -> Result<rd_core::RemoteJob> {
        writer::request(&self.writer, |reply| PluginsCommand::AdvanceRemoteJob {
            id,
            input: Box::new(input),
            reply,
        })
        .await
    }

    /// One remote job by its own identifier.
    pub async fn remote_job(&self, id: rd_core::RemoteJobId) -> Result<Option<rd_core::RemoteJob>> {
        crate::remote_job_store::get(&self.readers, id).await
    }

    /// The remote job one account already has for a content key, if any (RD-107-06).
    pub async fn remote_job_by_content(
        &self,
        account_id: rd_core::AccountId,
        content_key: &str,
    ) -> Result<Option<rd_core::RemoteJob>> {
        crate::remote_job_store::by_content(&self.readers, account_id, content_key).await
    }

    /// Every remote job of one account, newest first.
    pub async fn remote_jobs(
        &self,
        account_id: rd_core::AccountId,
    ) -> Result<Vec<rd_core::RemoteJob>> {
        crate::remote_job_store::list(&self.readers, account_id).await
    }

    /// Every remote job this installation knows about, newest first (RD-108-04).
    pub async fn all_remote_jobs(&self) -> Result<Vec<rd_core::RemoteJob>> {
        crate::remote_job_store::list_all(&self.readers).await
    }

    /// Removes one remote job's row, leaving what the provider holds untouched (RD-108-04).
    ///
    /// The other half of the pair ADR 0003 insists on keeping apart: deleting at the provider
    /// is `RemoteJobService::discard`, reached only from an explicit confirmed request, and
    /// nothing on this path calls it.
    pub async fn delete_remote_job(&self, id: rd_core::RemoteJobId) -> Result<bool> {
        writer::request(&self.writer, |reply| PluginsCommand::DeleteRemoteJob {
            id,
            reply,
        })
        .await
    }

    /// Every remote job whose next poll is due (RD-107-06).
    pub async fn due_remote_jobs(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::RemoteJob>> {
        crate::remote_job_store::due(&self.readers, now).await
    }

    /// The source a remote job was claimed with, so a restart offers the very same bytes.
    pub async fn remote_job_source(&self, id: rd_core::RemoteJobId) -> Result<Option<Vec<u8>>> {
        crate::remote_job_store::source(&self.readers, id).await
    }

    /// Lists every managed tool version this installation installed itself (RD-102-02).
    pub async fn list_managed_tools(&self) -> Result<Vec<ManagedToolRecord>> {
        managed_tools_store::list_managed_tools(&self.readers).await
    }

    /// How far the signed tool manifest has advanced, or `None` before the first refresh.
    ///
    /// The sequence is the replay floor: `rd_sign::replay::check` only refuses a replayed
    /// manifest if the caller remembers what it has already accepted.
    pub async fn tool_manifest_state(&self) -> Result<Option<ToolManifestState>> {
        managed_tools_store::tool_manifest_state(&self.readers).await
    }

    /// Records a verified, installed tool version.
    pub async fn record_managed_tool(&self, input: NewManagedTool) -> Result<ManagedToolRecord> {
        writer::request(&self.writer, |reply| PluginsCommand::RecordManagedTool {
            input,
            reply,
        })
        .await
    }

    /// Forgets one installed tool version; returns whether a row was removed.
    pub async fn forget_managed_tool(&self, name: String, version: String) -> Result<bool> {
        writer::request(&self.writer, |reply| PluginsCommand::ForgetManagedTool {
            name,
            version,
            reply,
        })
        .await
    }

    /// Raises the accepted tool-manifest sequence after a manifest verified.
    pub async fn accept_tool_manifest(&self, sequence: i64, issued_at: String) -> Result<()> {
        writer::request(&self.writer, |reply| PluginsCommand::AcceptToolManifest {
            sequence,
            issued_at,
            reply,
        })
        .await
    }

    /// Lists plugin signing keys the user confirmed on first use.
    pub async fn list_plugin_trusted_keys(&self) -> Result<Vec<PluginTrustedKey>> {
        plugin_keys_store::list_plugin_trusted_keys(&self.readers).await
    }

    /// Records a confirmed plugin signing key so installed packages still verify on restart.
    pub async fn trust_plugin_key(&self, input: NewPluginTrustedKey) -> Result<PluginTrustedKey> {
        writer::request(&self.writer, |reply| PluginsCommand::TrustPluginKey {
            input,
            reply,
        })
        .await
    }

    /// Revokes a plugin signing key; returns whether one was removed.
    pub async fn revoke_plugin_key(&self, key_id: String) -> Result<bool> {
        writer::request(&self.writer, |reply| PluginsCommand::RevokePluginKey {
            key_id,
            reply,
        })
        .await
    }

    /// Every withdrawn plugin package digest, newest first.
    ///
    /// Read once at start and used to replace the verifier's in-memory set; the check itself
    /// happens in process on every load, so nothing asks this per digest.
    pub async fn list_plugin_digest_revocations(&self) -> Result<Vec<PluginDigestRevocation>> {
        plugin_revocations_store::list_plugin_digest_revocations(&self.readers).await
    }

    /// Withdraws one exact package version so it is refused the next time plugins load.
    pub async fn revoke_plugin_digest(
        &self,
        input: NewPluginDigestRevocation,
    ) -> Result<PluginDigestRevocation> {
        writer::request(&self.writer, |reply| PluginsCommand::RevokePluginDigest {
            input,
            reply,
        })
        .await
    }

    /// Takes a withdrawal back; returns whether one was removed.
    pub async fn unrevoke_plugin_digest(&self, digest: String) -> Result<bool> {
        writer::request(&self.writer, |reply| PluginsCommand::UnrevokePluginDigest {
            digest,
            reply,
        })
        .await
    }

    /// Every plugin's version choice (RD-140-02), read once at start and by the inventory.
    pub async fn list_plugin_version_choices(&self) -> Result<Vec<PluginVersionChoice>> {
        plugin_versions_store::list_plugin_version_choices(&self.readers).await
    }

    /// One plugin's version choice, if it has one.
    pub async fn plugin_version_choice(
        &self,
        plugin_id: &str,
    ) -> Result<Option<PluginVersionChoice>> {
        plugin_versions_store::plugin_version_choice(&self.readers, plugin_id).await
    }

    /// Replaces one plugin's version choice; it takes effect at the next start.
    pub async fn save_plugin_version_choice(
        &self,
        input: NewPluginVersionChoice,
    ) -> Result<PluginVersionChoice> {
        writer::request(&self.writer, |reply| {
            PluginsCommand::SavePluginVersionChoice { input, reply }
        })
        .await
    }
}

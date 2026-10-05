//! Database facade for LinkGrabber packages, ordering and online-check bookkeeping.

use anyhow::Result;
use rd_core::{
    BatchId, CandidateId, CollectorBatch, CollectorPackage, CollectorPackageId, GrabberEntryRef,
    LinkCandidate, LinkCandidateState, LinkCheckResult,
};

use crate::{
    Database,
    archive_password::PasswordTable,
    collector_packages::{self, CollectorPackageChange, MoveTarget},
    collector_store::NewCollectorBatch,
    commands::{CollectorCommand, PackagesCommand},
    writer,
};

impl Database {
    /// Persists one submission grouped into packages.
    ///
    /// **The fragment goes into the vault before the address is shortened** (RD-110-38).
    /// Every intake path in the application ends up here, so this is the one place the rule
    /// is applied, and it is applied only to a host an installed plugin manifest declared:
    /// `rd_provider_registry::fragment_is_secret` answers from that table and from nothing
    /// else, so no service is a special case in the code. For every other address this is a
    /// pair of cheap lookups that change nothing.
    ///
    /// The archive password a package gets goes into the vault once the batch is in, one entry
    /// per package (RD-190-04); the packages come back carrying it. A vault that refuses costs
    /// the packages their password, logged, not the intake — the links are in by then.
    pub async fn add_collector_batch(
        &self,
        intake: NewCollectorBatch,
    ) -> Result<(CollectorBatch, Vec<CollectorPackage>, Vec<LinkCandidate>)> {
        let secret_fragment_refs = self.vault_fragments(&intake.urls).await;
        let (batch, mut packages, candidates, passwords) =
            writer::request(&self.writer, |reply| CollectorCommand::AddCollectorBatch {
                intake,
                secret_fragment_refs,
                reply,
            })
            .await?;
        if passwords.is_empty() {
            return Ok((batch, packages, candidates));
        }
        let stored = self
            .store_archive_passwords(
                PasswordTable::CollectorPackages,
                passwords
                    .iter()
                    .map(|(id, password)| (id.to_string(), Some(password.clone())))
                    .collect(),
            )
            .await;
        match stored {
            Err(error) => {
                tracing::warn!(%error, batch_id = %batch.id, "the archive passwords of a LinkGrabber batch could not be put in the vault");
            }
            Ok(()) => {
                for package in &mut packages {
                    if let Some((_, password)) = passwords
                        .iter()
                        .find(|(id, password)| *id == package.id && !password.is_empty())
                    {
                        package.has_password = true;
                        package.password = Some(password.clone());
                    }
                }
            }
        }
        Ok((batch, packages, candidates))
    }

    /// Puts each declared link's fragment away and returns the references, parallel to `urls`.
    ///
    /// A link whose provider declared nothing, a link with no fragment, and every link at all
    /// when no vault is installed produce `None` — and `None` is exactly the behaviour
    /// RD-109-32 had: the fragment is dropped one step later, by `rd_core::candidate_url`.
    /// A vault write that fails is logged without its subject and treated the same way, so a
    /// failing keyring costs the key rather than the whole intake.
    async fn vault_fragments(&self, urls: &[url::Url]) -> Vec<Option<String>> {
        let mut references = vec![None; urls.len()];
        let Some(vault) = self.secret_vault() else {
            return references;
        };
        for (index, url) in urls.iter().enumerate() {
            let declared = rd_provider_registry::fragment_is_secret(url);
            let (_, secret) = rd_core::split_candidate_url(url, declared);
            let Some(secret) = secret else { continue };
            match vault.put_bytes(secret.as_bytes()).await {
                Ok(reference) => references[index] = Some(reference),
                Err(error) => tracing::warn!(
                    %error,
                    host = url.host_str().unwrap_or_default(),
                    "a link fragment could not be vaulted; the link keeps no key"
                ),
            }
        }
        references
    }

    /// Lists LinkGrabber packages in display order (packages without open links are hidden),
    /// with their archive passwords (RD-104-04).
    pub async fn list_collector_packages(&self) -> Result<Vec<CollectorPackage>> {
        let mut packages = collector_packages::list(&self.readers).await?;
        self.reveal_archive_passwords(&mut packages).await;
        Ok(packages)
    }

    pub async fn get_collector_package(
        &self,
        id: CollectorPackageId,
    ) -> Result<Option<CollectorPackage>> {
        let mut package = collector_packages::get(&self.readers, id).await?;
        if let Some(package) = &mut package {
            self.reveal_archive_passwords(std::slice::from_mut(package))
                .await;
        }
        Ok(package)
    }

    /// Archive password of a LinkGrabber package, read from the vault (RD-190-04).
    pub async fn collector_package_password(
        &self,
        id: CollectorPackageId,
    ) -> Result<Option<String>> {
        self.archive_password(PasswordTable::CollectorPackages, id.to_string())
            .await
    }

    /// Applies a change to LinkGrabber packages; a password goes into the vault, one entry per
    /// package, after the other fields are written (RD-190-04).
    pub async fn update_collector_packages(
        &self,
        ids: Vec<CollectorPackageId>,
        mut change: CollectorPackageChange,
    ) -> Result<Vec<CollectorPackage>> {
        let password = change.password.take();
        let ids_for_password = password.as_ref().map(|_| ids.clone());
        let mut updated = writer::request(&self.writer, |reply| {
            CollectorCommand::UpdateCollectorPackages { ids, change, reply }
        })
        .await?;
        if let (Some(password), Some(ids)) = (password, ids_for_password) {
            let entries = ids
                .iter()
                .map(|id| (id.to_string(), password.clone()))
                .collect();
            self.store_archive_passwords(PasswordTable::CollectorPackages, entries)
                .await?;
            for package in &mut updated {
                package.has_password = password.as_deref().is_some_and(|value| !value.is_empty());
                package.password = None;
            }
        }
        self.reveal_archive_passwords(&mut updated).await;
        Ok(updated)
    }

    pub async fn reorder_collector_packages(&self, ids: Vec<CollectorPackageId>) -> Result<()> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::ReorderCollectorPackages { ids, reply }
        })
        .await
    }

    /// Writes the LinkGrabber's manual order across collector packages and NZB imports at once.
    ///
    /// Both kinds share one position sequence, so they can only be numbered together. `after` is
    /// the entry the listed ones are placed behind, `None` the head of the list; see
    /// `collector_packages::reorder_entries` for what a partial, unknown or unanchored list means.
    pub async fn reorder_grabber_entries(
        &self,
        entries: Vec<GrabberEntryRef>,
        after: Option<GrabberEntryRef>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::ReorderGrabberEntries {
                entries,
                after,
                reply,
            }
        })
        .await
    }

    pub async fn reorder_candidates(
        &self,
        package_id: CollectorPackageId,
        ids: Vec<CandidateId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| CollectorCommand::ReorderCandidates {
            package_id,
            ids,
            reply,
        })
        .await
    }

    pub async fn move_candidates(
        &self,
        ids: Vec<CandidateId>,
        target: MoveTarget,
    ) -> Result<CollectorPackage> {
        let mut package = writer::request(&self.writer, |reply| CollectorCommand::MoveCandidates {
            ids,
            target,
            reply,
        })
        .await?;
        // A package the move emptied is gone, and its password with it (RD-190-04).
        self.sweep_archive_passwords().await;
        self.reveal_archive_passwords(std::slice::from_mut(&mut package))
            .await;
        Ok(package)
    }

    pub async fn delete_collector_package(&self, id: CollectorPackageId) -> Result<()> {
        // Read before the delete: the reference lives in the row, so after the delete there
        // is nothing left to find the vault entry by (RD-110-38).
        let orphaned = crate::collector_store::package_vault_refs(&self.readers, id)
            .await
            .unwrap_or_default();
        writer::request(&self.writer, |reply| {
            CollectorCommand::DeleteCollectorPackage { id, reply }
        })
        .await?;
        self.forget_secrets(orphaned).await;
        // Its archive password was released by the delete itself (RD-190-04).
        self.sweep_archive_passwords().await;
        Ok(())
    }

    /// Re-derives automatically named packages after an online check learned file names.
    pub async fn regroup_collector_batches(&self, batch_ids: Vec<BatchId>) -> Result<()> {
        writer::request(&self.writer, |reply| CollectorCommand::RegroupBatches {
            batch_ids,
            reply,
        })
        .await?;
        self.sweep_archive_passwords().await;
        Ok(())
    }

    pub async fn claim_candidates_for_check(
        &self,
        ids: Vec<CandidateId>,
    ) -> Result<Vec<LinkCandidate>> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::ClaimCandidatesForCheck { ids, reply }
        })
        .await
    }

    pub async fn record_candidate_check(
        &self,
        id: CandidateId,
        result: Option<LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
        was_duplicate: bool,
        cached_by: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::RecordCandidateCheck {
                id,
                result,
                error,
                was_duplicate,
                cached_by,
                reply,
            }
        })
        .await
    }

    /// Marks a candidate as having no available check source, keeping the reason.
    ///
    /// `cached_by` stamps a cache answer next to it (RD-130-11): another provider holds the
    /// file although nothing here can check the link.
    pub async fn mark_candidate_unsupported(
        &self,
        id: CandidateId,
        message: rd_core::CandidateMessage,
        cached_by: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::MarkCandidateUnsupported {
                id,
                message,
                cached_by,
                reply,
            }
        })
        .await
    }

    /// Appends probed playlist entries as media candidates of a package.
    pub async fn add_media_candidates(
        &self,
        package_id: CollectorPackageId,
        entries: Vec<rd_core::MediaCandidate>,
    ) -> Result<Vec<LinkCandidate>> {
        writer::request(&self.writer, |reply| CollectorCommand::AddMediaCandidates {
            package_id,
            entries,
            reply,
        })
        .await
    }

    /// Switches the selected media variant of a candidate.
    pub async fn set_candidate_media_variant(
        &self,
        id: CandidateId,
        variant_id: String,
    ) -> Result<LinkCandidate> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::SetCandidateMediaVariant {
                id,
                variant_id,
                reply,
            }
        })
        .await
    }

    /// Re-routes a candidate to another provider (RD-080-06).
    pub async fn set_candidate_provider(
        &self,
        id: rd_core::CandidateId,
        provider: String,
    ) -> Result<rd_core::LinkCandidate> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::CollectorCommand::SetCandidateProvider {
                id,
                provider,
                reply,
            }
        })
        .await
    }

    /// Sets the cookie/authentication profile a candidate is queued with (RD-080-04).
    pub async fn set_candidate_auth_profile(
        &self,
        id: rd_core::CandidateId,
        selection: rd_core::AuthProfileSelection,
    ) -> Result<rd_core::LinkCandidate> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::CollectorCommand::SetCandidateAuthProfile {
                id,
                selection,
                reply,
            }
        })
        .await
    }

    /// Stores the fields an enricher plugin contributed to a candidate (RD-090-14).
    pub async fn set_candidate_enrichment(
        &self,
        id: CandidateId,
        fields: Vec<rd_core::EnrichmentField>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::SetCandidateEnrichment { id, fields, reply }
        })
        .await
    }

    /// What the indexer declared about the hit behind a candidate (RD-107-02).
    ///
    /// Empty for every link no subscription produced. Read on the way to an enricher, which
    /// applies the `attributes.rs` gate once more before anything leaves the process.
    pub async fn candidate_source_attributes(
        &self,
        id: CandidateId,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let mut connection = self.readers.acquire().await?;
        crate::collector_store::source_attributes(&mut connection, id).await
    }

    /// Carries a package's enricher fields from its candidates onto its queue rows
    /// (RD-107-02).
    ///
    /// Idempotent: a replace, so a retried enqueue writes the same rows.
    pub async fn carry_enrichment(
        &self,
        package_id: rd_core::PackageId,
        package_fields: Vec<rd_core::EnrichmentField>,
        files: Vec<(rd_core::DownloadId, Vec<rd_core::EnrichmentField>)>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| PackagesCommand::CarryEnrichment {
            package_id,
            package_fields,
            files,
            reply,
        })
        .await
    }

    /// Stores the probed format inventory of a media candidate (RD-080-01).
    pub async fn set_candidate_media_inventory(
        &self,
        id: CandidateId,
        state: rd_core::MediaCandidateState,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::SetCandidateMediaInventory {
                id,
                state: Box::new(state),
                reply,
            }
        })
        .await
    }

    /// Stores a resolved format selection on a media candidate (RD-080-01).
    pub async fn set_candidate_media_selection(
        &self,
        id: CandidateId,
        update: rd_core::MediaSelectionUpdate,
    ) -> Result<LinkCandidate> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::SetCandidateMediaSelection {
                id,
                update: Box::new(update),
                reply,
            }
        })
        .await
    }

    /// The stored format inventory and criteria of a media candidate.
    pub async fn candidate_media_state(
        &self,
        id: CandidateId,
    ) -> Result<Option<rd_core::MediaCandidateState>> {
        let mut connection = self.readers.acquire().await?;
        crate::collector_media::candidate_media_state(&mut connection, id).await
    }

    pub async fn set_candidate_file_name(
        &self,
        id: CandidateId,
        file_name: String,
    ) -> Result<LinkCandidate> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::SetCandidateFileName {
                id,
                file_name,
                reply,
            }
        })
        .await
    }

    /// Locks every enqueueable link of a package — or, with `only`, those of them it names —
    /// and returns them with their previous states.
    pub async fn claim_package_for_enqueue(
        &self,
        id: CollectorPackageId,
        only: Option<Vec<CandidateId>>,
    ) -> Result<Vec<(LinkCandidate, LinkCandidateState)>> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::ClaimPackageForEnqueue { id, only, reply }
        })
        .await
    }

    pub async fn finish_package_enqueue(
        &self,
        id: CollectorPackageId,
        success: bool,
        restore: Vec<(CandidateId, LinkCandidateState)>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::FinishPackageEnqueue {
                id,
                success,
                restore,
                reply,
            }
        })
        .await?;
        self.sweep_archive_passwords().await;
        Ok(())
    }
}

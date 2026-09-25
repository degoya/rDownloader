//! Database facade for LinkGrabber packages, ordering and online-check bookkeeping.

use anyhow::Result;
use rd_core::{
    BatchId, CandidateId, CollectorBatch, CollectorPackage, CollectorPackageId, GrabberEntryRef,
    LinkCandidate, LinkCandidateState, LinkCheckResult, MirrorPreference,
};

use crate::{
    Database,
    collector_packages::{self, CollectorPackageChange, MoveTarget},
    collector_store::NewCollectorBatch,
    commands::WriterCommand,
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
    pub async fn add_collector_batch(
        &self,
        intake: NewCollectorBatch,
    ) -> Result<(CollectorBatch, Vec<CollectorPackage>, Vec<LinkCandidate>)> {
        let secret_fragment_refs = self.vault_fragments(&intake.urls).await;
        writer::request(&self.writer, |reply| WriterCommand::AddCollectorBatch {
            intake,
            secret_fragment_refs,
            reply,
        })
        .await
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

    /// Lists LinkGrabber packages in display order (packages without open links are hidden).
    pub async fn list_collector_packages(&self) -> Result<Vec<CollectorPackage>> {
        collector_packages::list(&self.readers).await
    }

    pub async fn get_collector_package(
        &self,
        id: CollectorPackageId,
    ) -> Result<Option<CollectorPackage>> {
        collector_packages::get(&self.readers, id).await
    }

    /// Archive password of a LinkGrabber package (never serialized).
    pub async fn collector_package_password(
        &self,
        id: CollectorPackageId,
    ) -> Result<Option<String>> {
        collector_packages::password(&self.readers, id).await
    }

    pub async fn update_collector_packages(
        &self,
        ids: Vec<CollectorPackageId>,
        change: CollectorPackageChange,
    ) -> Result<Vec<CollectorPackage>> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpdateCollectorPackages { ids, change, reply }
        })
        .await
    }

    pub async fn reorder_collector_packages(&self, ids: Vec<CollectorPackageId>) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ReorderCollectorPackages { ids, reply }
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
        writer::request(&self.writer, |reply| WriterCommand::ReorderGrabberEntries {
            entries,
            after,
            reply,
        })
        .await
    }

    pub async fn reorder_candidates(
        &self,
        package_id: CollectorPackageId,
        ids: Vec<CandidateId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ReorderCandidates {
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
        writer::request(&self.writer, |reply| WriterCommand::MoveCandidates {
            ids,
            target,
            reply,
        })
        .await
    }

    pub async fn delete_collector_package(&self, id: CollectorPackageId) -> Result<()> {
        // Read before the delete: the reference lives in the row, so after the delete there
        // is nothing left to find the vault entry by (RD-110-38).
        let orphaned = crate::collector_store::package_secret_fragment_refs(&self.readers, id)
            .await
            .unwrap_or_default();
        writer::request(&self.writer, |reply| {
            WriterCommand::DeleteCollectorPackage { id, reply }
        })
        .await?;
        self.forget_secrets(orphaned).await;
        Ok(())
    }

    /// Re-derives automatically named packages after an online check learned file names.
    pub async fn regroup_collector_batches(&self, batch_ids: Vec<BatchId>) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::RegroupBatches {
            batch_ids,
            reply,
        })
        .await
    }

    /// The standing mirror preference, or its defaults when none was ever stored (RD-110-19).
    pub async fn mirror_preference(&self) -> Result<MirrorPreference> {
        Ok(self
            .get_setting(crate::MIRROR_PREFERENCE_KEY)
            .await?
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default())
    }

    /// Stores the standing mirror preference and re-chooses every group under it.
    pub async fn set_mirror_preference(&self, preference: MirrorPreference) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetMirrorPreference {
            preference,
            reply,
        })
        .await
    }

    /// Makes one candidate its group's chosen mirror, or releases that choice.
    ///
    /// `false` means the link belongs to no mirror group, so there was nothing to choose
    /// between.
    pub async fn set_mirror_pin(&self, id: CandidateId, pinned: bool) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::SetMirrorPin {
            id,
            pinned,
            reply,
        })
        .await
    }

    /// Takes a proposed mirror group apart, so its links are single candidates again.
    ///
    /// The refusal is stored as pairs of links, not as an absent group, so it survives the
    /// recompute at intake, after the online check and on a move between packages.
    pub async fn dissolve_mirror_group(&self, id: CandidateId) -> Result<crate::MirrorDissolve> {
        writer::request(&self.writer, |reply| WriterCommand::DissolveMirrorGroup {
            id,
            reply,
        })
        .await
    }

    pub async fn claim_candidates_for_check(
        &self,
        ids: Vec<CandidateId>,
    ) -> Result<Vec<LinkCandidate>> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ClaimCandidatesForCheck { ids, reply }
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
        writer::request(&self.writer, |reply| WriterCommand::RecordCandidateCheck {
            id,
            result,
            error,
            was_duplicate,
            cached_by,
            reply,
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
            WriterCommand::MarkCandidateUnsupported {
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
        writer::request(&self.writer, |reply| WriterCommand::AddMediaCandidates {
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
            WriterCommand::SetCandidateMediaVariant {
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
            crate::commands::WriterCommand::SetCandidateProvider {
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
            crate::commands::WriterCommand::SetCandidateAuthProfile {
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
            WriterCommand::SetCandidateEnrichment { id, fields, reply }
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
        writer::request(&self.writer, |reply| WriterCommand::CarryEnrichment {
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
            WriterCommand::SetCandidateMediaInventory {
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
            WriterCommand::SetCandidateMediaSelection {
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
        writer::request(&self.writer, |reply| WriterCommand::SetCandidateFileName {
            id,
            file_name,
            reply,
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
            WriterCommand::ClaimPackageForEnqueue { id, only, reply }
        })
        .await
    }

    pub async fn finish_package_enqueue(
        &self,
        id: CollectorPackageId,
        success: bool,
        restore: Vec<(CandidateId, LinkCandidateState)>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::FinishPackageEnqueue {
            id,
            success,
            restore,
            reply,
        })
        .await
    }
}

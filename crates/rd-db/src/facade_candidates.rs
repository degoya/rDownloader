//! Database facade for LinkGrabber candidates and their listings, package edits and ordering,
//! secret fragments and stream-transform keys.

use anyhow::Result;

use crate::{
    Database,
    archive_password::PasswordTable,
    collector_store,
    commands::{CollectorCommand, NetworkCommand, PackagesCommand},
    writer,
};

impl Database {
    /// Lists LinkGrabber batches newest first.
    pub async fn list_collector_batches(&self) -> Result<Vec<rd_core::CollectorBatch>> {
        collector_store::list_batches(&self.readers).await
    }

    /// One page of [`Self::list_collector_batches`], cut by SQLite, and how many batches there
    /// are (RD-191-05): `offset` rows skipped, then at most `limit` (`None`: the rest).
    pub async fn collector_batches_page(
        &self,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<(Vec<rd_core::CollectorBatch>, u64)> {
        collector_store::batches_page(&self.readers, offset, limit).await
    }

    /// Lists LinkGrabber candidates newest first.
    pub async fn list_candidates(&self) -> Result<Vec<rd_core::LinkCandidate>> {
        collector_store::list_candidates(&self.readers).await
    }

    /// One page of [`Self::list_candidates`], cut by SQLite, and how many links that list holds
    /// (RD-191-05).
    pub async fn candidates_page(
        &self,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<(Vec<rd_core::LinkCandidate>, u64)> {
        collector_store::candidates_page(&self.readers, offset, limit).await
    }

    /// One page of [`Self::list_collector_packages`], cut by SQLite, and how many packages that
    /// list holds (RD-191-05); only the page's passwords are read from the vault.
    pub async fn collector_packages_page(
        &self,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<(Vec<rd_core::CollectorPackage>, u64)> {
        let (mut packages, total) =
            crate::collector_packages::page(&self.readers, offset, limit).await?;
        self.reveal_archive_passwords(&mut packages).await;
        self.fill_queue_names(&mut packages).await;
        Ok((packages, total))
    }

    /// Loads a LinkGrabber candidate.
    pub async fn get_candidate(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::LinkCandidate>> {
        collector_store::get_candidate(&self.readers, id).await
    }

    /// Applies category (with destination) and/or priority to packages.
    ///
    /// A password goes into the vault, one entry per package, after the other fields are
    /// written (RD-190-04); the packages come back with it.
    pub async fn update_packages(
        &self,
        ids: Vec<rd_core::PackageId>,
        mut change: crate::package_store::PackageChange,
    ) -> Result<Vec<rd_core::DownloadPackage>> {
        let password = change.password.take();
        let ids_for_password = password.as_ref().map(|_| ids.clone());
        let mut updated = writer::request(&self.writer, |reply| PackagesCommand::UpdatePackages {
            ids,
            change,
            reply,
        })
        .await?;
        if let (Some(password), Some(ids)) = (password, ids_for_password) {
            let entries = ids
                .iter()
                .map(|id| (id.to_string(), password.clone()))
                .collect();
            self.store_archive_passwords(PasswordTable::Packages, entries)
                .await?;
            for package in &mut updated {
                package.has_password = password.as_deref().is_some_and(|value| !value.is_empty());
                package.password = None;
            }
        }
        self.reveal_archive_passwords(&mut updated).await;
        Ok(updated)
    }

    /// Renames a package and points it at a new folder beside its old one (RD-106-13).
    ///
    /// One transaction: the name, the destination, the `previous_destination` the disk move
    /// resumes from, and every absolute path stored for this package. `None` when the id is
    /// unknown.
    pub async fn rename_package_directory(
        &self,
        id: rd_core::PackageId,
        name: String,
        destination: String,
    ) -> Result<Option<rd_core::DownloadPackage>> {
        let mut renamed = writer::request(&self.writer, |reply| {
            PackagesCommand::RenamePackageDirectory {
                id,
                name,
                destination,
                reply,
            }
        })
        .await?;
        if let Some(package) = &mut renamed {
            self.reveal_archive_passwords(std::slice::from_mut(package))
                .await;
        }
        Ok(renamed)
    }

    /// Where the package's files lived before its last category change, while the sweep of
    /// that directory is still outstanding.
    pub async fn package_previous_destination(
        &self,
        id: rd_core::PackageId,
    ) -> Result<Option<String>> {
        crate::package_store::previous_destination(&self.readers, id).await
    }

    /// Marks the outstanding sweep of a package's former directory as done.
    pub async fn clear_package_previous_destination(&self, id: rd_core::PackageId) -> Result<()> {
        writer::request(&self.writer, |reply| {
            PackagesCommand::ClearPreviousDestination { id, reply }
        })
        .await
    }

    /// Points a package at the folder a torrent move carried its files to (RD-1100-10), with
    /// every absolute path stored for it, but only while it still names `from`.
    ///
    /// `false` when it no longer does; nothing is changed then. No `previous_destination` is
    /// recorded: the move has its own journal, and the scheduler's sweep stays out of it.
    pub async fn switch_package_destination(
        &self,
        id: rd_core::PackageId,
        from: String,
        to: String,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            PackagesCommand::SwitchPackageDestination {
                id,
                from,
                to,
                reply,
            }
        })
        .await
    }

    /// Stores a manual queue order (positions 1..n in the given order).
    pub async fn reorder_packages(&self, ids: Vec<rd_core::PackageId>) -> Result<()> {
        writer::request(&self.writer, |reply| PackagesCommand::ReorderPackages {
            ids,
            reply,
        })
        .await
    }

    /// Stores a manual file order inside one package (positions 1..n in the given order).
    ///
    /// `ids` has to name exactly the package's files; the API layer rejects anything else, and
    /// the `UPDATE` ignores an id that belongs elsewhere.
    pub async fn reorder_downloads(
        &self,
        package_id: rd_core::PackageId,
        ids: Vec<rd_core::DownloadId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| PackagesCommand::ReorderDownloads {
            package_id,
            ids,
            reply,
        })
        .await
    }

    /// Removes one LinkGrabber candidate that is not currently being enqueued.
    ///
    /// Its vaulted link fragment (RD-110-38) and captured request body go with it. Read before
    /// the delete, because the references are columns of the row being removed: a secret
    /// nothing points at is a leak with a delay.
    pub async fn delete_candidate(&self, id: rd_core::CandidateId) -> Result<()> {
        let orphaned = crate::collector_store::candidate_vault_refs(&self.readers, id)
            .await
            .unwrap_or_default();
        writer::request(&self.writer, |reply| CollectorCommand::DeleteCandidate {
            id,
            reply,
        })
        .await?;
        self.forget_secrets(orphaned).await;
        // A package the candidate leaves empty goes, and its archive password (RD-190-04).
        self.sweep_archive_passwords().await;
        Ok(())
    }

    /// Removes all visible LinkGrabber candidates and returns the affected count.
    ///
    /// The same predicate as the delete itself, so a candidate that is kept -- one being
    /// resolved, one already enqueued -- keeps its secret too.
    pub async fn delete_candidates(&self) -> Result<u64> {
        let orphaned = crate::collector_store::deletable_vault_refs(&self.readers)
            .await
            .unwrap_or_default();
        let removed = writer::request(&self.writer, |reply| CollectorCommand::DeleteCandidates {
            reply,
        })
        .await?;
        self.forget_secrets(orphaned).await;
        self.sweep_archive_passwords().await;
        Ok(removed)
    }

    /// The `vault://` reference one LinkGrabber candidate holds, if any (RD-110-38).
    ///
    /// The enqueue reads it to move ownership onto the download row. The reference, never
    /// the fragment: what can open it is the vault, and only where the key is needed.
    pub async fn candidate_secret_fragment_ref(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<String>> {
        crate::collector_store::secret_fragment_ref(&self.readers, id).await
    }

    /// The reference a download inherited, and the fragment behind it (RD-110-38).
    ///
    /// The one call that hands back the plaintext, and it is made in exactly one place: the
    /// scheduler, immediately before it asks the stream-transform plugin what lies behind the
    /// address. `None` when the download has no reference, when no vault is installed, or
    /// when the vault refuses -- the resolve then fails on the plugin's own terms rather
    /// than on a half-restored address.
    pub async fn download_secret_fragment(
        &self,
        id: rd_core::DownloadId,
    ) -> Result<Option<String>> {
        use sqlx::Row;

        let row = sqlx::query("SELECT secret_fragment_ref FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.readers)
            .await?;
        let Some(reference) = row.and_then(|row| {
            row.try_get::<Option<String>, _>("secret_fragment_ref")
                .ok()
                .flatten()
        }) else {
            return Ok(None);
        };
        let Some(vault) = self.secret_vault() else {
            return Ok(None);
        };
        let bytes = vault.get_bytes(&reference).await?;
        Ok(Some(String::from_utf8(bytes)?))
    }

    /// The vault reference this download's transform key is reached by, putting the key away
    /// the first time (RD-120-11, ADR 0011).
    ///
    /// The description a stream-transform plugin answers with carries the key itself and no
    /// reference; `rd-http` refuses to build a transform out of one that has no reference,
    /// because the reference is what the fingerprint stands the key on. This is the one place
    /// that closes that gap, and it is deliberately *idempotent*: a second attempt at the same
    /// file gets the same reference back, so the fingerprint is the same and the chunk MACs
    /// the first attempt wrote are still recognisably its own.
    ///
    /// A key that differs from the stored one is a different file behind the same row -- a
    /// re-resolve that came back with another node, or a link whose key was corrected. The old
    /// entry is then removed and a new reference written, which changes the fingerprint and
    /// makes the continuation start over rather than decrypt with one key over bytes written
    /// with another.
    ///
    /// `Ok(None)` means no vault is installed. The caller then has no reference to put in the
    /// description and the transform is refused, which is the right answer: key material that
    /// cannot be put away must not be carried around instead.
    pub async fn adopt_transform_key(
        &self,
        id: rd_core::DownloadId,
        key: &[u8],
    ) -> Result<Option<String>> {
        let Some(vault) = self.secret_vault() else {
            return Ok(None);
        };
        let existing = self.download_transform_key_ref(id).await?;
        if let Some(reference) = &existing
            && vault
                .get_bytes(reference)
                .await
                .is_ok_and(|stored| stored == key)
        {
            return Ok(Some(reference.clone()));
        }
        let reference = vault.put_bytes(key).await?;
        self.set_download_transform_key_ref(id, Some(reference.clone()))
            .await?;
        if let Some(stale) = existing {
            self.forget_secrets(vec![stale]).await;
        }
        Ok(Some(reference))
    }

    /// The reference a download row holds, without opening the vault.
    pub async fn download_transform_key_ref(
        &self,
        id: rd_core::DownloadId,
    ) -> Result<Option<String>> {
        use sqlx::Row;

        let row = sqlx::query("SELECT transform_key_ref FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.readers)
            .await?;
        Ok(row.and_then(|row| {
            row.try_get::<Option<String>, _>("transform_key_ref")
                .ok()
                .flatten()
        }))
    }

    /// Writes -- or clears -- that reference.
    pub(crate) async fn set_download_transform_key_ref(
        &self,
        id: rd_core::DownloadId,
        reference: Option<String>,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::DownloadsCommand::SetTransformKeyRef {
                id,
                reference,
                reply,
            }
        })
        .await
    }

    /// The reviewed remote directory listing of one link candidate.
    pub async fn candidate_listing(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::RemoteCandidateState>> {
        crate::remote_store::candidate_listing(&self.readers, id).await
    }

    /// Stores a probed listing on a candidate, preserving an existing selection for the
    /// same directory root.
    pub async fn set_candidate_listing(
        &self,
        id: rd_core::CandidateId,
        listing: rd_core::RemoteListing,
        credential_id: Option<rd_core::RemoteCredentialId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| NetworkCommand::SetCandidateListing {
            id,
            listing: Box::new(listing),
            credential_id,
            reply,
        })
        .await
    }

    /// Replaces the file selection of one candidate and returns the resolved listing.
    pub async fn set_candidate_listing_plan(
        &self,
        id: rd_core::CandidateId,
        plan: rd_core::RemoteListingPlan,
    ) -> Result<rd_core::ResolvedRemoteListing> {
        writer::request(&self.writer, |reply| {
            NetworkCommand::SetCandidateListingPlan { id, plan, reply }
        })
        .await
    }
}

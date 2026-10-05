//! The checks of links that are no hoster links: media pages, FTP, SFTP and WebDAV servers and
//! object storage buckets.

use super::*;

impl LinkCheckService {
    /// The client a check of `url` goes out with: held to `guard` when the link was proposed.
    pub(super) async fn client_for(
        &self,
        url: &url::Url,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> Result<rd_scheduler::NetworkClient> {
        match guard {
            Some(policy) => {
                self.inner
                    .scheduler
                    .guarded_client(url, policy.clone())
                    .await
            }
            None => self.inner.scheduler.direct_client(url).await,
        }
    }

    pub(super) async fn check_media(
        &self,
        candidate: LinkCandidate,
        guard: Option<&rd_http::AddressPolicy>,
    ) {
        // A direct manifest is classified first: whether it is protected, and whether it is
        // live, decides where it goes before the extractor is asked anything.
        if self.classify_manifest(&candidate, true, guard).await {
            return;
        }
        match self.inner.media_probe.probe(&candidate.url).await {
            Ok(mut entries) if !entries.is_empty() => {
                let first = entries.remove(0);
                let file_name = rd_db::media_file_name(&first.info);
                let size = first
                    .info
                    .selected_variant()
                    .and_then(|variant| variant.filesize_approx)
                    .and_then(|value| rd_core::ByteCount::new(value).ok());
                let inventory = first.state.clone();
                let result = LinkCheckResult {
                    url: candidate.url.clone(),
                    status: LinkStatus::Online,
                    file_name: Some(file_name),
                    size,
                    media: Some(first.info),
                };
                self.record(&candidate, Some(result), None).await;
                // The bounded variant list travels with the check result; the full
                // inventory is stored separately so it never rides in a list response.
                if let Err(error) = self
                    .inner
                    .database
                    .set_candidate_media_inventory(candidate.id, inventory)
                    .await
                {
                    tracing::warn!(candidate_id = %candidate.id, %error, "media inventory could not be stored");
                }
                if !entries.is_empty()
                    && let Some(package_id) = candidate.package_id
                    && let Err(error) = self
                        .inner
                        .database
                        .add_media_candidates(package_id, entries)
                        .await
                {
                    tracing::warn!(%error, "playlist entries could not be added");
                }
            }
            Ok(_) => {
                self.record(
                    &candidate,
                    None,
                    Some(rd_core::CandidateMessage::coded(
                        "collector.check_no_media",
                        "No media found on this page",
                    )),
                )
                .await;
            }
            Err(failure) => {
                self.record(&candidate, None, Some((&failure).into())).await;
            }
        }
    }

    /// Resolves an ftp/sftp/webdav link into the listing that is reviewed before queueing.
    ///
    /// This is the one online check that genuinely contacts the server and enumerates it,
    /// which is affordable here because a directory listing is cheap on all three
    /// protocols — unlike crawling a gallery or joining a torrent swarm.
    pub(super) async fn check_remote(
        &self,
        candidate: LinkCandidate,
        guard: Option<&rd_http::AddressPolicy>,
    ) {
        if rd_core::ObjectStorageProvider::from_scheme(candidate.url.scheme()).is_some() {
            self.check_object(candidate).await;
            return;
        }
        let Some(target) = rd_core::RemoteTarget::parse(&candidate.url) else {
            self.record(
                &candidate,
                None,
                Some(rd_core::CandidateMessage::coded(
                    "collector.check_remote_invalid",
                    "The address is not a valid remote link",
                )),
            )
            .await;
            return;
        };
        let probed = match target.protocol.family() {
            rd_core::RemoteFamily::Ftp => self
                .inner
                .ftp
                .probe(&target, candidate.remote_credential_id, guard)
                .await
                .map(|(probed, credential)| {
                    (
                        match probed {
                            rd_ftp::Probed::Resolved(listing) => Ok(listing),
                            rd_ftp::Probed::Failed(failure) => Err(failure),
                        },
                        credential,
                    )
                }),
            rd_core::RemoteFamily::Sftp => self
                .inner
                .sftp
                .probe(&target, candidate.remote_credential_id, guard)
                .await
                .map(|(probed, credential)| {
                    (
                        match probed {
                            rd_sftp::Probed::Resolved(listing) => Ok(listing),
                            rd_sftp::Probed::Failed(failure) => Err(failure),
                        },
                        credential,
                    )
                }),
            rd_core::RemoteFamily::Webdav => self.probe_webdav(&target, guard).await,
        };
        let (outcome, credential_id) = match probed {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(candidate_id = %candidate.id, %error, "remote link check failed");
                self.record(
                    &candidate,
                    None,
                    Some(rd_core::CandidateMessage::coded(
                        "collector.check_remote_unreachable",
                        "The server could not be reached",
                    )),
                )
                .await;
                return;
            }
        };
        let listing = match outcome {
            Ok(listing) => listing,
            // A blocked host key or a missing login is a coded failure the UI can act on,
            // so the code travels with the message rather than being flattened to text.
            Err(failure) => {
                self.record_failure(&candidate, &failure).await;
                return;
            }
        };
        let summary = listing.summary();
        if let Err(error) = self
            .inner
            .database
            .set_candidate_listing(candidate.id, *listing, credential_id)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %error, "listing could not be stored");
        }
        let result = LinkCheckResult {
            url: candidate.url.clone(),
            status: LinkStatus::Online,
            // A single-file link keeps its own name; a directory is named by its folder.
            file_name: (summary.single_file || summary.file_count > 0)
                .then(|| file_name_for(&candidate, &summary)),
            size: Some(summary.total_bytes),
            media: None,
        };
        self.record(&candidate, Some(result), None).await;
    }

    /// Resolves an object storage link (RD-150-04): one object, or the prefix it names.
    ///
    /// The profile is chosen here as it will be at the transfer's start, so a link that no
    /// profile serves is marked before anybody queues it.
    async fn check_object(&self, candidate: LinkCandidate) {
        let listing = match self.inner.object_storage.probe(&candidate.url).await {
            Ok(Ok(listing)) => listing,
            Ok(Err(failure)) => {
                self.record_failure(&candidate, &failure).await;
                return;
            }
            Err(error) => {
                tracing::warn!(candidate_id = %candidate.id, %error, "object storage link check failed");
                self.record(
                    &candidate,
                    None,
                    Some(rd_core::CandidateMessage::coded(
                        "collector.check_remote_unreachable",
                        "The server could not be reached",
                    )),
                )
                .await;
                return;
            }
        };
        let summary = listing.summary();
        if let Err(error) = self
            .inner
            .database
            .set_candidate_listing(candidate.id, listing, None)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %error, "listing could not be stored");
        }
        let result = LinkCheckResult {
            url: candidate.url.clone(),
            status: LinkStatus::Online,
            file_name: (summary.single_file || summary.file_count > 0)
                .then(|| file_name_for(&candidate, &summary)),
            size: Some(summary.total_bytes),
            media: None,
        };
        self.record(&candidate, Some(result), None).await;
    }

    /// WebDAV probes through the same pooled client the transfer will use, so the auth
    /// profile, proxy and custom CA that apply to the download apply to the check too — and,
    /// for a link a document or a page proposed, the address rule (RD-150-03).
    async fn probe_webdav(
        &self,
        target: &rd_core::RemoteTarget,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> anyhow::Result<(
        Result<Box<rd_core::RemoteListing>, rd_core::Failure>,
        Option<rd_core::RemoteCredentialId>,
    )> {
        let Some(scope) = target.sanitized_url() else {
            return Ok((
                Err(rd_core::Failure::coded(
                    rd_core::FailureKind::Permanent,
                    rd_webdav::PROPFIND_FAILED,
                    "The WebDAV address is not valid",
                )),
                None,
            ));
        };
        let network = self.client_for(&scope, guard).await?;
        let probed = rd_webdav::probe(&network.client, target).await?;
        Ok((
            match probed {
                rd_webdav::Probed::Resolved(listing) => Ok(listing),
                rd_webdav::Probed::Failed(failure) => Err(failure),
            },
            None,
        ))
    }
}

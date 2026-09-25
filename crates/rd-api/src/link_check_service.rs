//! Background online check for LinkGrabber candidates (provider API or direct probe).

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use rd_core::{Account, BatchId, CandidateId, LinkCandidate, LinkCheckResult, LinkStatus};
use rd_db::Database;
use rd_scheduler::SchedulerHandle;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{hosters, link_check_cache, link_check_probe::probe_direct};

enum CheckJob {
    Candidates(Vec<CandidateId>),
    Batch(BatchId),
}

struct Inner {
    database: Database,
    scheduler: SchedulerHandle,
    media_probe: Arc<dyn rd_media::MediaProbe>,
    ftp: rd_ftp::FtpService,
    sftp: rd_sftp::SftpService,
    /// Keeps a re-routed torrent the check read for its download (RD-130-18).
    torrent: rd_torrent::TorrentService,
    jobs: mpsc::Sender<CheckJob>,
    /// Announces a finished batch check. Lets a caller act on the outcome — the subscription
    /// auto-queue does — without the check service having to know who is waiting.
    completed: broadcast::Sender<BatchId>,
    cancellation: CancellationToken,
    /// Where the enricher plugins live, and the host they reach the outside world through.
    plugins: rd_plugin_host::PluginInstaller,
    plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    /// Installed metadata enrichers (RD-090-14), compiled on first use. An installation with
    /// none — or with enrichment switched off — never pays for compiling one.
    enrichers: tokio::sync::OnceCell<Arc<rd_plugin_ext::MetadataEnrichers>>,
    /// The remote-job plugins asked whether a provider's cache holds a link (RD-130-11).
    /// Attached after start, because that service is built with this one; unset, no cache
    /// is asked and the check is what it was before.
    cache_checkers: std::sync::OnceLock<crate::remote_job_service::RemoteJobService>,
}

/// Cloneable handle; checks run sequentially on one background task.
#[derive(Clone)]
pub struct LinkCheckService {
    inner: Arc<Inner>,
}

impl LinkCheckService {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        database: Database,
        scheduler: SchedulerHandle,
        media_probe: Arc<dyn rd_media::MediaProbe>,
        ftp: rd_ftp::FtpService,
        sftp: rd_sftp::SftpService,
        torrent: rd_torrent::TorrentService,
        plugins: rd_plugin_host::PluginInstaller,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(512);
        let service = Self {
            inner: Arc::new(Inner {
                database,
                scheduler,
                media_probe,
                ftp,
                sftp,
                torrent,
                jobs: sender,
                completed: broadcast::channel(64).0,
                cancellation: CancellationToken::new(),
                plugins,
                plugin_host,
                enrichers: tokio::sync::OnceCell::new(),
                cache_checkers: std::sync::OnceLock::new(),
            }),
        };
        tokio::spawn(service.clone().run(receiver));
        service
    }

    /// Checks the given candidates (ignored while the service is shutting down).
    pub async fn check(&self, ids: Vec<CandidateId>) {
        if !ids.is_empty() {
            let _ = self.inner.jobs.send(CheckJob::Candidates(ids)).await;
        }
    }

    /// Checks every open candidate of a batch and regroups its packages afterwards.
    pub async fn check_batch(&self, batch_id: BatchId) {
        let _ = self.inner.jobs.send(CheckJob::Batch(batch_id)).await;
    }

    /// Lets the check ask the remote-job plugins' caches (RD-130-11). Only the first call
    /// counts.
    pub fn attach_cache_checkers(&self, checkers: crate::remote_job_service::RemoteJobService) {
        let _ = self.inner.cache_checkers.set(checkers);
    }

    /// Receives the id of every batch whose check has finished, successfully or not.
    #[must_use]
    pub fn subscribe_completed(&self) -> broadcast::Receiver<BatchId> {
        self.inner.completed.subscribe()
    }

    pub fn shutdown(&self) {
        self.inner.cancellation.cancel();
    }

    async fn run(self, mut receiver: mpsc::Receiver<CheckJob>) {
        loop {
            let job = tokio::select! {
                () = self.inner.cancellation.cancelled() => return,
                job = receiver.recv() => match job {
                    Some(job) => job,
                    None => return,
                },
            };
            let batch = match &job {
                CheckJob::Batch(id) => Some(*id),
                CheckJob::Candidates(_) => None,
            };
            if let Err(error) = self.process(job).await {
                tracing::warn!(%error, "link check failed");
            }
            // Announced even when the check failed: the candidates have reached a final state
            // either way, and a waiter that never hears back is worse than one that finds
            // nothing enqueueable.
            if let Some(batch) = batch {
                let _ = self.inner.completed.send(batch);
            }
        }
    }

    async fn process(&self, job: CheckJob) -> Result<()> {
        let (ids, batch_id) = match job {
            CheckJob::Candidates(ids) => (ids, None),
            CheckJob::Batch(batch_id) => (
                self.inner
                    .database
                    .list_candidates()
                    .await?
                    .into_iter()
                    .filter(|candidate| candidate.batch_id == batch_id)
                    .map(|candidate| candidate.id)
                    .collect(),
                Some(batch_id),
            ),
        };
        let claimed = self.inner.database.claim_candidates_for_check(ids).await?;
        if claimed.is_empty() {
            return Ok(());
        }
        let accounts: Vec<Account> = self
            .inner
            .database
            .list_accounts()
            .await?
            .into_iter()
            .filter(|account| account.enabled)
            .collect();
        let resolvers = self.inner.scheduler.resolvers();
        let mut catalogues = Vec::with_capacity(accounts.len());
        for account in &accounts {
            catalogues.push((account.id, hosters::catalogue(&resolvers, account.id).await));
        }
        // Which provider's cache holds what, asked before any link is checked and merged into
        // each result as it is written (RD-130-11).
        let hints = match self.inner.cache_checkers.get() {
            Some(checkers) => {
                link_check_cache::hints(checkers, &self.inner.database, &claimed, &accounts).await
            }
            None => HashMap::new(),
        };

        // The candidate travels with whether *its own* hoster has an account, because that is
        // what an inconclusive answer has to be read against: a check that ran through a
        // covering multihoster, or through no account of this hoster at all, is inconclusive
        // for a reason the reader can do something about (RD-109-43).
        let mut by_account: HashMap<rd_core::AccountId, Vec<(LinkCandidate, bool)>> =
            HashMap::new();
        let mut direct = Vec::new();
        let mut unsupported = Vec::new();
        let mut media = Vec::new();
        let mut gallery = Vec::new();
        let mut remote = Vec::new();
        for candidate in claimed {
            let provider = candidate.provider.as_deref().unwrap_or("direct_http");
            if provider == rd_core::MEDIA_PROVIDER {
                media.push(candidate);
                continue;
            }
            if matches!(
                provider,
                rd_core::FTP_PROVIDER | rd_core::SFTP_PROVIDER | rd_core::WEBDAV_PROVIDER
            ) {
                remote.push(candidate);
                continue;
            }
            if provider == rd_core::GALLERY_PROVIDER
                || provider == rd_core::TORRENT_PROVIDER
                // An NZB is fetched and parsed at enqueue time, not probed here: reading it
                // twice would double the load on an indexer for no new information.
                || provider == rd_core::NZB_PROVIDER
                // A live manifest was already probed and classified by an earlier check;
                // re-opening the stream would only tell us what we know (RD-080-06).
                || provider == rd_core::RECORD_PROVIDER
            {
                gallery.push(candidate);
                continue;
            }
            let own_account = accounts
                .iter()
                .find(|account| account.provider.eq_ignore_ascii_case(provider))
                .map(|account| account.id);
            let covering_account = catalogues
                .iter()
                .find(|(_, hosters)| hosters::supports(hosters, &candidate.url))
                .map(|(id, _)| *id);
            if let Some(account) = own_account.or(covering_account) {
                by_account
                    .entry(account)
                    .or_default()
                    .push((candidate, own_account.is_some()));
            } else if provider == "direct_http" {
                direct.push(candidate);
            } else {
                unsupported.push(candidate);
            }
        }
        for (account, candidates) in by_account {
            let checked_by = accounts
                .iter()
                .find(|known| known.id == account)
                .map(|known| known.provider.to_ascii_lowercase());
            let urls: Vec<url::Url> = candidates.iter().map(|(c, _)| c.url.clone()).collect();
            match self.inner.scheduler.resolvers().check(account, urls).await {
                Ok(results) => {
                    for (candidate, own_account) in candidates {
                        let result = results.iter().find(|r| r.url == candidate.url).cloned();
                        let message = provider_message(result.as_ref(), own_account);
                        self.record_with_cache(
                            &candidate,
                            result,
                            message,
                            hints.get(&candidate.id),
                            checked_by.as_deref(),
                        )
                        .await;
                    }
                }
                Err(failure) => {
                    // The whole batch failed: the failure already carries its own code, and
                    // flattening it to prose here would throw that away.
                    for (candidate, _) in candidates {
                        self.record(&candidate, None, Some((&failure).into())).await;
                    }
                }
            }
        }
        if !direct.is_empty() {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
            let mut tasks = Vec::with_capacity(direct.len());
            for candidate in direct {
                let permit = Arc::clone(&semaphore).acquire_owned().await?;
                let scheduler = self.inner.scheduler.clone();
                tasks.push(tokio::spawn(async move {
                    let _permit = permit;
                    let result = match scheduler.direct_client(&candidate.url).await {
                        Ok(network) => Some(
                            probe_direct(&network.client, &network.headers, candidate.url.clone())
                                .await,
                        ),
                        Err(_) => None,
                    };
                    (candidate, result)
                }));
            }
            for task in tasks {
                if let Ok((candidate, result)) = task.await {
                    // An indexer link often hides the extension behind a query — NZBHydra
                    // and Prowlarr both do — so the declared content type is what identifies
                    // it (RD-080-11). Asked first since RD-130-18: a torrent served as
                    // `application/octet-stream` is plausible as a manifest too, and reading
                    // it for one would be a grab of its own.
                    let rerouted = self.reclassify_document(&candidate).await;
                    // A manifest does not have to announce itself in its address: a signed
                    // CDN link has no extension and is often served as `text/plain`
                    // (RD-080-06). Reclassifying here is what keeps it from being queued as
                    // a text file.
                    if matches!(rerouted, Rerouted::No)
                        && result.is_some()
                        && self.classify_manifest(&candidate, false).await
                    {
                        continue;
                    }
                    let result = rerouted_document(&rerouted, result);
                    let message = direct_message(result.as_ref());
                    self.record(&candidate, result, message).await;
                }
            }
        }
        for candidate in media {
            self.check_media(candidate).await;
        }
        for candidate in remote {
            self.check_remote(candidate).await;
        }
        // Enumerating a gallery means crawling it, and probing a torrent means joining the
        // swarm — both cost as much as the download, so these links pass without a probe.
        for candidate in gallery {
            let result = LinkCheckResult {
                url: candidate.url.clone(),
                status: LinkStatus::Online,
                file_name: None,
                size: None,
                media: None,
            };
            // A cache that holds the torrent or the NZB raises it to `cached` (RD-130-11).
            self.record_with_cache(
                &candidate,
                Some(result),
                None,
                hints.get(&candidate.id),
                None,
            )
            .await;
        }
        // These links were never checked, so they must not be presented as online: checking
        // a hoster link needs an account. Downloading it may still work when a resolver
        // offers a free path, so the message distinguishes the two cases.
        for candidate in unsupported {
            let message = if resolvers.has_free_resolver(&candidate.url) {
                rd_core::CandidateMessage::coded(
                    "collector.check_not_checked_free",
                    "Not checked (no account); a free download will be attempted",
                )
            } else {
                rd_core::CandidateMessage::coded(
                    "collector.check_no_source",
                    "No check source for this hoster (account missing)",
                )
            };
            // Another provider's cache may hold the file all the same (RD-130-11): the state and
            // the message stay, and the answer is stamped next to them.
            let cached_by = link_check_cache::unsupported_cached_by(hints.get(&candidate.id));
            if let Err(problem) = self
                .inner
                .database
                .mark_candidate_unsupported(candidate.id, message, cached_by)
                .await
            {
                tracing::warn!(candidate_id = %candidate.id, %problem, "could not store link check");
            }
        }
        if let Some(batch_id) = batch_id {
            self.inner
                .database
                .regroup_collector_batches(vec![batch_id])
                .await?;
        }
        Ok(())
    }

    /// Probes a media page; playlists fan out into additional candidates of the package.
    /// Classifies a direct HLS/DASH manifest before it is handed to the extractor
    /// (RD-080-06).
    ///
    /// Returns `true` when the candidate has been dealt with here — refused as DRM, or
    /// routed to the recorder as a live stream — and `false` when it is an ordinary VOD the
    /// yt-dlp probe should describe as usual.
    /// `force` skips the cheap gate below, for a link whose address already ends in
    /// `.m3u8`/`.mpd` and is therefore worth reading in full.
    async fn classify_manifest(&self, candidate: &LinkCandidate, force: bool) -> bool {
        let Ok(network) = self.inner.scheduler.direct_client(&candidate.url).await else {
            return false;
        };
        // Reading the body of every direct link to see whether it might be a playlist would
        // mean downloading a slice of every file in the queue. A HEAD first keeps that to
        // the responses that could plausibly be one: a manifest media type, or something
        // text-ish and small enough to be a document rather than a video.
        if !force {
            let probed = rd_http::probe_with_headers(
                &network.client,
                candidate.url.clone(),
                &network.headers,
            )
            .await;
            let Ok(probed) = probed else { return false };
            if !manifest_plausible(probed.content_type.as_deref(), probed.total_bytes) {
                return false;
            }
        }
        // Verbatim, not `peek_body_text`: that one strips tags and collapses whitespace to
        // summarise a hoster's HTML error page, which would leave a playlist as one line and
        // an MPD as nothing at all.
        let body = rd_http::fetch_text_verbatim(
            &network.client,
            candidate.url.clone(),
            &network.headers,
            rd_media::MAX_MANIFEST_BYTES,
        )
        .await;
        let Some(body) = body else { return false };
        let Some(kind) =
            rd_media::detect_manifest(&body.final_url, body.content_type.as_deref(), &body.text)
        else {
            return false;
        };
        // Relative URIs resolve against where the body actually came from, not against the
        // address that was pasted: a redirect to another host would otherwise make every
        // segment URL point at the wrong origin.
        let base = &body.final_url;
        let report = match kind {
            rd_media::ManifestKind::Hls => rd_media::parse_hls(&body.text, base),
            rd_media::ManifestKind::Dash => match rd_media::parse_dash(&body.text, base) {
                Ok(report) => report,
                Err(_) => return false,
            },
        };

        // A protected stream is a stated non-goal, so it ends here rather than as an obscure
        // ffmpeg failure ten minutes into a download. Deliberately without a code: the
        // protection scheme is the whole content of the sentence and no catalogue can carry
        // it, the same reason the host-key fingerprint stays untranslated below.
        if let Some(reason) = report.drm {
            tracing::info!(
                candidate_id = %candidate.id,
                detail = %reason.detail(),
                "manifest is DRM-protected; refusing"
            );
            self.record(
                candidate,
                None,
                Some(rd_core::CandidateMessage::plain(format!(
                    "DRM-protected stream ({})",
                    reason.detail()
                ))),
            )
            .await;
            return true;
        }

        // Credentials belong to the manifest's own origin. A CDN on a second hostname is
        // ordinary, but it does not inherit this link's session.
        let foreign = report.foreign_origins(base);
        if !foreign.is_empty() {
            tracing::debug!(
                candidate_id = %candidate.id,
                count = foreign.len(),
                "manifest references other origins; credentials stay with the manifest host"
            );
        }

        // A live manifest has no end, so it belongs to the recorder. Handing it to the file
        // downloader produces a job that can never reach 100 %.
        if report.class == rd_media::ManifestClass::Live {
            tracing::info!(candidate_id = %candidate.id, "manifest is live; routing to the recorder");
            if let Err(error) = self
                .inner
                .database
                .set_candidate_provider(candidate.id, rd_core::RECORD_PROVIDER.to_owned())
                .await
            {
                tracing::warn!(%error, "live manifest could not be routed to the recorder");
                return false;
            }
            let result = LinkCheckResult {
                url: candidate.url.clone(),
                status: LinkStatus::Online,
                file_name: None,
                size: None,
                media: None,
            };
            self.record(candidate, Some(result), None).await;
            return true;
        }
        false
    }

    /// Re-routes a direct link the server declares to be an NZB or a torrent
    /// (RD-080-11).
    ///
    /// A HEAD is enough: the content type is the whole signal, and fetching the document
    /// here would mean asking the indexer for it twice. Both Newznab and Torznab hand out
    /// download links with the identity in the header rather than the path, and so do the
    /// NZBHydra and Prowlarr proxies in front of them.
    ///
    /// Answers whether the link was re-routed, which the caller needs since RD-110-07: a
    /// small XML document is not `looks_downloadable`, and an NZB that has just been sent to
    /// its import path must not also be recorded as a page that is not a file.
    ///
    /// A torrent is read here as well (RD-120-68): its `info.name` is the release name, and
    /// the address it came from ends in whatever token the site hands out. The file tree is
    /// stored on the candidate the way an uploaded `.torrent` stores it, so the review and
    /// the info hash exist before the row does.
    async fn reclassify_document(&self, candidate: &LinkCandidate) -> Rerouted {
        let Ok(network) = self.inner.scheduler.direct_client(&candidate.url).await else {
            return Rerouted::No;
        };
        let Ok(probed) =
            rd_http::probe_with_headers(&network.client, candidate.url.clone(), &network.headers)
                .await
        else {
            return Rerouted::No;
        };
        let essence = probed
            .content_type
            .as_deref()
            .map(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or(value)
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let mut sniffed_torrent = None;
        let provider = if rd_core::NZB_CONTENT_TYPES.contains(&essence.as_str()) {
            rd_core::NZB_PROVIDER
        } else if rd_core::TORRENT_CONTENT_TYPES.contains(&essence.as_str()) {
            rd_core::TORRENT_PROVIDER
        } else if let Some(sniffed) = crate::link_check_probe::sniff_document(
            &network.client,
            &network.headers,
            &candidate.url,
        )
        .await
        {
            match sniffed {
                crate::link_check_probe::Sniffed::Nzb => rd_core::NZB_PROVIDER,
                crate::link_check_probe::Sniffed::Torrent(bytes) => {
                    sniffed_torrent = bytes;
                    rd_core::TORRENT_PROVIDER
                }
            }
        } else {
            // Logged rather than swallowed: this is where an indexer link that ends up being
            // downloaded as a document is decided, and the content type it answered with is
            // the one thing needed to work out why.
            tracing::debug!(
                url = %rd_core::redact_url(&candidate.url),
                content_type = %essence,
                "link was not recognised as a container"
            );
            return Rerouted::No;
        };
        tracing::info!(
            url = %rd_core::redact_url(&candidate.url),
            provider,
            "link re-routed to its import path"
        );
        if let Err(error) = self
            .inner
            .database
            .set_candidate_provider(candidate.id, provider.to_owned())
            .await
        {
            tracing::warn!(%error, provider, "link could not be re-routed to its import path");
        }
        if provider != rd_core::TORRENT_PROVIDER {
            return Rerouted::Container;
        }
        let parsed = crate::link_check_probe::read_torrent(
            &network.client,
            &network.headers,
            candidate,
            &self.inner.torrent,
            sniffed_torrent,
        )
        .await;
        if let Some(parsed) = &parsed
            && let Err(error) = self
                .inner
                .database
                .set_candidate_torrent_state(
                    candidate.id,
                    rd_core::TorrentCandidateState::ready(parsed.metadata.clone()),
                )
                .await
        {
            tracing::warn!(%error, "file tree of a re-routed torrent could not be stored");
        }
        Rerouted::Torrent(parsed.map(Box::new))
    }

    async fn check_media(&self, candidate: LinkCandidate) {
        // A direct manifest is classified first: whether it is protected, and whether it is
        // live, decides where it goes before the extractor is asked anything.
        if self.classify_manifest(&candidate, true).await {
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
    async fn check_remote(&self, candidate: LinkCandidate) {
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
                .probe(&target, candidate.remote_credential_id)
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
                .probe(&target, candidate.remote_credential_id)
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
            rd_core::RemoteFamily::Webdav => self.probe_webdav(&candidate, &target).await,
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

    /// WebDAV probes through the same pooled client the transfer will use, so the auth
    /// profile, proxy and custom CA that apply to the download apply to the check too.
    async fn probe_webdav(
        &self,
        candidate: &LinkCandidate,
        target: &rd_core::RemoteTarget,
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
        let network = self
            .inner
            .scheduler
            .network_client(None, None, rd_core::AuthProfileSelection::Auto, &scope)
            .await?;
        let probed = rd_webdav::probe(&network.client, target).await?;
        let _ = candidate;
        Ok((
            match probed {
                rd_webdav::Probed::Resolved(listing) => Ok(listing),
                rd_webdav::Probed::Failed(failure) => Err(failure),
            },
            None,
        ))
    }

    /// Stores a failure on the candidate.
    ///
    /// A candidate carries a plain message rather than a code, so the parameters a person
    /// actually needs are folded into the text: an unknown SSH host key is useless without
    /// the fingerprint to compare, which is public data, not a secret.
    async fn record_failure(&self, candidate: &LinkCandidate, failure: &rd_core::Failure) {
        let mut message = rd_core::CandidateMessage::from(failure);
        if let Some(fingerprint) = failure.params.get("fingerprint") {
            message.text.push_str(" (");
            message.text.push_str(fingerprint);
            if let Some(stored) = failure.params.get("stored_fingerprint") {
                message.text.push_str(", previously ");
                message.text.push_str(stored);
            }
            message.text.push(')');
            // The fingerprint is in the text and in no catalogue, so the translated sentence
            // would drop exactly the part this message exists for.
            message.code = None;
        }
        self.record(candidate, None, Some(message)).await;
    }

    async fn record(
        &self,
        candidate: &LinkCandidate,
        result: Option<LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
    ) {
        self.record_with_cache(candidate, result, error, None, None)
            .await;
    }

    /// Like [`Self::record`], with what a provider's cache said about the link merged in
    /// first (RD-130-11). `checked_by` is the slug of the account whose resolver checked it,
    /// which a resolver's own `cached` answer is stamped with.
    async fn record_with_cache(
        &self,
        candidate: &LinkCandidate,
        result: Option<LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
        hint: Option<&link_check_cache::CacheHint>,
        checked_by: Option<&str>,
    ) {
        let (result, cached_by) = link_check_cache::merge(result, hint, checked_by);
        let error = if result.is_some() {
            error
        } else {
            error.or_else(|| {
                Some(rd_core::CandidateMessage::coded(
                    "collector.check_failed",
                    "The check failed",
                ))
            })
        };
        // `candidate` carries the pre-claim state: a duplicate keeps its warning but still
        // gains the probed file name and media metadata.
        let was_duplicate = candidate.state == rd_core::LinkCandidateState::Duplicate;
        let online = result
            .as_ref()
            .is_some_and(|result| matches!(result.status, LinkStatus::Online | LinkStatus::Cached));
        let known = result
            .as_ref()
            .and_then(|result| result.media.as_ref())
            .and_then(|media| serde_json::to_string(media).ok());
        let file_name = result.as_ref().and_then(|result| result.file_name.clone());
        if let Err(problem) = self
            .inner
            .database
            .record_candidate_check(candidate.id, result, error, was_duplicate, cached_by)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %problem, "could not store link check");
        }
        if online {
            self.enrich(candidate, file_name.as_deref(), known.as_deref())
                .await;
        }
    }

    /// Asks the installed enrichers about a link that resolved.
    ///
    /// Only after the check succeeded, and only when somebody switched enrichment on: an
    /// enricher reaches a service outside this machine, and doing that for every media link
    /// on the strength of having installed a plugin would be a decision nobody made.
    async fn enrich(
        &self,
        candidate: &LinkCandidate,
        file_name: Option<&str>,
        known: Option<&str>,
    ) {
        // The switch is checked before the plugins are compiled: an installation that never
        // asked for enrichment should not pay for building an enricher it will not call.
        if !self.enrichment_enabled().await {
            return;
        }
        let enrichers = self.enrichers().await;
        if enrichers.is_empty() {
            return;
        }
        // What the indexer already declared about this hit, read back and gated once more
        // before it leaves the process (RD-107-02).
        let declared = match self
            .inner
            .database
            .candidate_source_attributes(candidate.id)
            .await
        {
            Ok(attributes) => attributes,
            Err(error) => {
                tracing::warn!(
                    candidate_id = %candidate.id,
                    %error,
                    "declared attributes unreadable"
                );
                std::collections::BTreeMap::new()
            }
        };
        let known = known_for(known, &declared);
        let fields = enrichers
            .enrich(&candidate.url, file_name, known.as_deref())
            .await;
        if fields.is_empty() {
            return;
        }
        if let Err(error) = self
            .inner
            .database
            .set_candidate_enrichment(candidate.id, fields)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %error, "enrichment could not be stored");
        }
    }

    /// The installed enrichers, compiled on first use.
    async fn enrichers(&self) -> Arc<rd_plugin_ext::MetadataEnrichers> {
        Arc::clone(
            self.inner
                .enrichers
                .get_or_init(|| async {
                    match rd_plugin_ext::MetadataEnrichers::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(enrichers) => Arc::new(enrichers),
                        Err(error) => {
                            tracing::warn!(%error, "could not load metadata enricher plugins");
                            Arc::new(rd_plugin_ext::MetadataEnrichers::none())
                        }
                    }
                })
                .await,
        )
    }

    /// Whether the person switched metadata enrichment on. Read per check rather than cached:
    /// switching it off has to take effect on the next link, not after a restart.
    async fn enrichment_enabled(&self) -> bool {
        // A database error still reads as "off", as before: enrichment is an optional extra
        // and must not fail a check. A field stored with the wrong type is reported.
        self.inner
            .database
            .service_setting_field::<bool>("metadata_enrichment_enabled")
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }
}

/// Name shown for a resolved remote link: the file for a single-file link, the folder
/// otherwise.
fn file_name_for(candidate: &LinkCandidate, summary: &rd_core::RemoteListingSummary) -> String {
    let from_path = candidate
        .url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|part| !part.is_empty()))
        .map(str::to_owned);
    from_path
        .or_else(|| {
            summary
                .root
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "download".to_owned())
}

/// The message a provider check leaves on a candidate.
///
/// Three unrelated situations used to share one sentence. `Some("Check result missing")` was
/// handed to every candidate of a provider batch and the store kept it for every result of
/// status `Unknown`, so the reader saw the same words whether the plugin had skipped the URL
/// or answered about it (RD-109-43) — and in the second, commoner case the words were simply
/// untrue, a result was there.
///
/// They are told apart here, and each carries a stable code the interface translates:
///
/// * no result for this URL — the plugin answered for the batch and said nothing about it;
/// * `Unknown` with an account for this hoster — the hoster itself could not say;
/// * `Unknown` without one — the case worth naming, because several hosters answer a check
///   only to an account of their own while the download works without one. The check ran
///   through a covering multihoster, or through an account of a different hoster.
///
/// A conclusive answer — online or gone — leaves no message at all.
fn provider_message(
    result: Option<&LinkCheckResult>,
    own_account: bool,
) -> Option<rd_core::CandidateMessage> {
    match result {
        None => Some(rd_core::CandidateMessage::coded(
            "collector.check_no_result",
            "The check returned no answer for this link",
        )),
        Some(result) if result.status == LinkStatus::Unknown => Some(if own_account {
            rd_core::CandidateMessage::coded(
                "collector.check_unknown",
                "The hoster could not say whether this link is still available",
            )
        } else {
            rd_core::CandidateMessage::coded(
                "collector.check_unknown_no_account",
                "The hoster could not say whether this link is still available; checking it \
                 needs an account for this hoster. The download may still work without one",
            )
        }),
        Some(_) => None,
    }
}

/// The same for a direct link, which is probed here rather than by a plugin.
///
/// `probe_direct` returns `Unknown` for everything that is neither a reply nor a 4xx — a
/// timeout, a refused connection, a proxy in the way. That answer used to be stored as "No
/// HTTP client available", which names the one situation it was not.
fn direct_message(result: Option<&LinkCheckResult>) -> Option<rd_core::CandidateMessage> {
    match result {
        None => Some(rd_core::CandidateMessage::coded(
            "collector.check_no_client",
            "No HTTP client was available for this link",
        )),
        Some(result) if result.status == LinkStatus::Unknown => {
            Some(rd_core::CandidateMessage::coded(
                "collector.check_inconclusive",
                "The server did not answer the check, so the link could not be confirmed",
            ))
        }
        // The one answer that is a statement about the *file* rather than about the check,
        // so it always carries its reason: a state nobody can queue out of has to say why.
        // *Which* reason it is depends on the installation and not on the response, and this
        // is the one place that has both answers at once -- see `unresolvable_message`.
        Some(result) if result.status == LinkStatus::Unresolvable => Some(unresolvable_message(
            rd_provider_registry::provider_for_url(&result.url).is_some(),
        )),
        Some(_) => None,
    }
}

/// Which of the two unresolvable cases a page response is (RD-120-18).
///
/// Every one of them used to be `collector.check_not_a_file`, "this address answers with a
/// page, not with a file". The owner's live check of 1.1 ended on that sentence four times --
/// `controlc.com`, `nfile.cc`, `dwp.la`, `icerbox.com` -- and not once was the address the
/// cause. The site rule had resolved correctly and handed over an address this installation
/// has no resolver for. The old sentence is literally true of a hoster's download page and
/// names the wrong culprit, so the reader goes looking at the rule, the site or their browser
/// instead of at the installation.
///
/// `known_hoster` is `rd_provider_registry::provider_for_url(...).is_some()`, and it is asked
/// for its **negative**. `None` means no installed plugin manifest claims this host, so
/// nothing in this installation can turn the address into a file. The positive is deliberately
/// *not* read as "this will work": a registered provider says the host is known, not that its
/// resolver is loaded, enabled, or able to serve this particular address. That is why `true`
/// keeps the older code -- a statement about the address, which promises nothing about the
/// installation.
///
/// **The new sentence stops where the evidence stops and does not say a plugin is missing.**
/// ADR 0019 measured the four cases afterwards: `nfile.cc` and `dwp.la` are not hosters at all
/// but affiliate cloakers, and `dwp.la` forwards to `downup.me`, which `plugins/xfs-generic`
/// already claims. For those two, "a plugin for this hoster is missing" would replace one
/// false diagnosis with another. What holds for all four is only this: no resolver is
/// installed for the host in the address. Telling a forwarder from a genuine dead end means
/// following the redirect, and nothing here can -- `probe_direct` keeps
/// `rd_http::ProbeResult::final_url` to itself and `LinkCheckResult` carries the declared
/// address alone. Following it at intake is a collector decision about untrusted redirects
/// with its own job, recorded as the open gap of RD-120-18.
///
/// The host is not written into the sentence here. It is the candidate's own address, which
/// the row already carries, and the catalogues name it as `{host}`; the message therefore
/// stays a stable code plus a parameter rather than prose assembled in Rust.
fn unresolvable_message(known_hoster: bool) -> rd_core::CandidateMessage {
    if known_hoster {
        rd_core::CandidateMessage::coded(
            "collector.check_not_a_file",
            "The address answered with a page rather than a file",
        )
    } else {
        rd_core::CandidateMessage::coded(
            "collector.check_no_resolver",
            "No resolver is installed for this host, so this address cannot be turned into a \
             file here",
        )
    }
}

/// Undoes the "not a file" verdict for a document that has just found its import path.
///
/// An NZB or a `.torrent` is XML or bencode a few kilobytes long, which is exactly what
/// `looks_downloadable` refuses -- rightly, because nothing should *download* it. It is not
/// downloaded: it was re-routed a line earlier and will be imported. Leaving the verdict
/// standing would park an indexer link in a state no one can queue out of (RD-080-11,
/// RD-110-07).
///
/// A torrent that could be read also gives the link its name and size (RD-120-68). The name
/// is recorded as declared, so the regroup after the check names the package after the
/// release rather than after the address's last segment -- an opaque download token that
/// once became a package, and with it a folder, called `JKs2Jt3Fo=_l3XcwZVDZXkBOYSX+nhd+A==`.
fn rerouted_document(
    rerouted: &Rerouted,
    result: Option<LinkCheckResult>,
) -> Option<LinkCheckResult> {
    let mut probed = result?;
    if !matches!(rerouted, Rerouted::No) && probed.status == LinkStatus::Unresolvable {
        probed.status = LinkStatus::Online;
    }
    if let Rerouted::Torrent(Some(torrent)) = rerouted {
        probed.file_name = Some(rd_files::sanitize_file_name(&torrent.name));
        probed.size = rd_core::ByteCount::new(torrent.total_bytes).ok();
    }
    Some(probed)
}

/// Where [`LinkCheckService::reclassify_document`] sent a link.
enum Rerouted {
    /// Not a container: the link stays on the download path.
    No,
    /// An NZB, imported under its own name later.
    Container,
    /// A torrent, with its metadata when the file could be read.
    Torrent(Option<Box<rd_torrent::ParsedTorrent>>),
}

/// Whether a response could be an HLS/DASH manifest, from its headers alone.
///
/// Deliberately generous on the type and strict on the size: CDNs serve playlists as
/// `text/plain` all the time, but no manifest is 4 GB, and reading the first megabytes of
/// every large file to find out would cost more than the check is worth.
fn manifest_plausible(content_type: Option<&str>, total_bytes: Option<u64>) -> bool {
    let essence = content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let declared = matches!(
        essence.as_str(),
        "application/vnd.apple.mpegurl"
            | "application/x-mpegurl"
            | "audio/mpegurl"
            | "audio/x-mpegurl"
            | "application/dash+xml"
    );
    if declared {
        return true;
    }
    let texty = matches!(
        essence.as_str(),
        "text/plain" | "application/xml" | "text/xml" | "application/octet-stream" | ""
    );
    texty
        && total_bytes.is_none_or(|bytes| bytes > 0 && bytes <= rd_media::MAX_MANIFEST_BYTES as u64)
}

/// Builds the `known` JSON an enricher is asked with (RD-107-02).
///
/// The decision this job made first: the indexer's attributes ride inside the JSON the WIT
/// contract already carries, under an added `indexer` key, rather than in a new record field.
/// A field added to a WIT record is not additive for a component already shipped — every
/// installed enricher would have to be rebuilt and re-signed before the host got an answer
/// out of it again — while an added JSON key is ignored by any reader that does not know it.
///
/// The resolved media metadata therefore stays exactly where it was, at the top level of the
/// object, and `indexer` joins it as a sibling.
///
/// The attributes pass `rd_subscription::retain_attributes` here, not only on the way into
/// the database: the rule is "nothing that gate discards reaches a plugin", and this is the
/// last place before it does. `retain` is idempotent, so gating an already gated map costs a
/// pass over at most 40 short strings and changes nothing.
///
/// A link with no declared attributes — every pasted, captured and container link — gets the
/// media JSON through unchanged, so a plugin sees precisely what it saw before this existed.
fn known_for(
    media_json: Option<&str>,
    declared: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let gated = rd_subscription::retain_attributes(declared, None).attributes;
    if gated.is_empty() {
        return media_json.map(str::to_owned);
    }
    let indexer = serde_json::Value::Object(
        gated
            .into_iter()
            .map(|(name, value)| (name, serde_json::Value::String(value)))
            .collect(),
    );
    let mut object = match media_json.map(serde_json::from_str::<serde_json::Value>) {
        Some(Ok(serde_json::Value::Object(object))) => object,
        // No media, or media that is not an object: the attributes still travel, and the
        // media JSON is not silently dropped — it keeps its own key instead.
        Some(Ok(other)) => {
            let mut object = serde_json::Map::new();
            object.insert("media".to_owned(), other);
            object
        }
        Some(Err(_)) | None => serde_json::Map::new(),
    };
    object.insert("indexer".to_owned(), indexer);
    serde_json::to_string(&serde_json::Value::Object(object)).ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rd_core::{LinkCheckResult, LinkStatus};

    use super::{
        Rerouted, direct_message, known_for, manifest_plausible, provider_message,
        rerouted_document, unresolvable_message,
    };

    /// Serialises the tests that write the process-wide provider registry, the way
    /// `providers_handlers` does for the same reason.
    static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A hoster row for `host`, the way an installed plugin's manifest contributes one.
    ///
    /// Nothing is compiled into the registry since RD-101-13, so "this host is supported"
    /// only exists in a test that installs it.
    fn hoster_row(host: &str) -> rd_provider_registry::DynamicProvider {
        rd_provider_registry::DynamicProvider {
            plugin_id: format!("plugin-{host}"),
            spec: rd_provider_registry::ProviderSpec {
                slug: host.replace('.', "-"),
                display_name: host.to_owned(),
                kind: rd_provider_registry::ProviderKind::Hoster,
                credentials: rd_provider_registry::CredentialKind::NoneRequired,
                username_required: false,
                transfer_auth: rd_provider_registry::TransferAuth::None,
                secrets: Vec::new(),
                request_domains: vec![host.to_owned()],
                cookie_scope: None,
                match_hosts: vec![host.to_owned()],
                host_aliases: Vec::new(),
                source: rd_provider_registry::ProviderSource::Plugin,
                plugin_id: Some(format!("plugin-{host}")),
                plugin_version: Some("1.0.0".to_owned()),
            },
        }
    }

    fn unresolvable_at(address: &str) -> LinkCheckResult {
        LinkCheckResult {
            url: address.parse().expect("URL"),
            status: LinkStatus::Unresolvable,
            file_name: None,
            size: None,
            media: None,
        }
    }

    /// The addresses behind the four cases the owner's live check of 1.1 ended on (RD-120-18).
    ///
    /// Every one of them reached `collector.check_not_a_file`, and in none of them was the
    /// address the cause: the site rule had resolved and handed over an address with no
    /// resolver. `downmagaz.net` and `avxhm.se` are release pages, so they stand here through
    /// the two addresses they hand over to. What those addresses *are* differs -- ADR 0019
    /// found `icerbox.com` to be an unsupported hoster and `nfile.cc` and `dwp.la` to be
    /// affiliate cloakers -- and one message covers all of them precisely because it claims
    /// nothing beyond the missing resolver, which is what this list pins down.
    const REPORTED_HOSTS: [&str; 5] = [
        "https://controlc.com/1a2b3c4d",
        "https://nfile.cc/abcdef123456",
        "https://dwp.la/abcdef123456",
        "https://icerbox.com/abcdef123456",
        "https://vipergirls.to/threads/1234567-a-release",
    ];

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_link_without_declared_attributes_is_asked_exactly_as_before() {
        // The criterion for a manually pasted link: nothing behind it, nothing added.
        let media = r#"{"title":"Some clip","duration_seconds":42}"#;
        assert_eq!(
            known_for(Some(media), &BTreeMap::new()).as_deref(),
            Some(media)
        );
        assert_eq!(known_for(None, &BTreeMap::new()), None);
    }

    #[test]
    fn the_indexer_attributes_join_the_media_metadata_as_a_sibling() {
        let media = r#"{"title":"Some clip","duration_seconds":42}"#;
        let known = known_for(
            Some(media),
            &map(&[("imdb", "tt0111161"), ("imdbscore", "9.3")]),
        )
        .expect("known built");
        let value: serde_json::Value = serde_json::from_str(&known).expect("valid json");
        // The media fields stay where an enricher built before this change looks for them.
        assert_eq!(value["title"], "Some clip");
        assert_eq!(value["duration_seconds"], 42);
        assert_eq!(value["indexer"]["imdb"], "tt0111161");
        assert_eq!(value["indexer"]["imdbscore"], "9.3");
    }

    #[test]
    fn attributes_reach_a_plugin_even_without_media_metadata() {
        let known = known_for(None, &map(&[("imdb", "tt0111161")])).expect("known built");
        let value: serde_json::Value = serde_json::from_str(&known).expect("valid json");
        assert_eq!(value["indexer"]["imdb"], "tt0111161");
    }

    #[test]
    fn nothing_the_attribute_gate_discards_reaches_a_plugin() {
        // Exactly the values `rd_subscription::attributes` refuses: a credential by name, a
        // credential inside a value, a cover address that is not an absolute http(s) URL, and
        // a real password written where the specification wants its flag.
        let known = known_for(
            Some(r#"{"title":"Release"}"#),
            &map(&[
                ("apikey", "deadbeefcafe"),
                ("passkey", "0123456789abcdef"),
                ("rsstoken", "sekrit-token"),
                (
                    "nfo",
                    "https://indexer.example/nfo?apikey=deadbeefcafe&id=7",
                ),
                ("coverurl", "data:image/png;base64,AAAA"),
                ("password", "hunter2"),
                ("imdbscore", "9.3"),
            ]),
        )
        .expect("known built");
        for secret in [
            "deadbeefcafe",
            "0123456789abcdef",
            "sekrit-token",
            "hunter2",
            "data:image",
        ] {
            assert!(!known.contains(secret), "{secret} leaked: {known}");
        }
        let value: serde_json::Value = serde_json::from_str(&known).expect("valid json");
        // The names whose value was a credential are gone entirely, not merely emptied.
        for name in ["apikey", "passkey", "rsstoken"] {
            assert!(value["indexer"].get(name).is_none(), "{name} kept");
        }
        // A real password becomes the specification's flag, never the secret.
        assert_eq!(value["indexer"]["password"], "1");
        // What is safe still arrives, or the gate would be a wall.
        assert_eq!(value["indexer"]["imdbscore"], "9.3");
    }

    fn probed(status: LinkStatus) -> LinkCheckResult {
        LinkCheckResult {
            url: "https://1fichier.com/?8x6wertoi51r8vptrojn"
                .parse()
                .expect("URL"),
            status,
            file_name: None,
            size: None,
            media: None,
        }
    }

    /// The defect RD-109-43 was raised for: one sentence for three different situations.
    ///
    /// Two links of two hosters stood in the LinkGrabber with "Check result missing" while
    /// the plugins had in fact answered — `Unknown` — and the download worked without any
    /// account. Nothing in the row said which of the three had happened, so nothing could be
    /// done about it.
    #[test]
    fn a_missing_answer_and_an_unclear_one_are_two_different_messages() {
        let missing = provider_message(None, true).expect("a message");
        let unclear = provider_message(Some(&probed(LinkStatus::Unknown)), true).expect("one");
        assert_eq!(missing.code.as_deref(), Some("collector.check_no_result"));
        assert_eq!(unclear.code.as_deref(), Some("collector.check_unknown"));
        assert_ne!(missing.code, unclear.code);
        assert_ne!(missing.text, unclear.text);
    }

    /// Without an account for the link's own hoster, the message says so.
    ///
    /// This is the situation the report described: the check needs an account, the download
    /// does not, and the row claimed neither.
    #[test]
    fn an_unclear_answer_without_an_account_names_the_account() {
        let message =
            provider_message(Some(&probed(LinkStatus::Unknown)), false).expect("a message");
        assert_eq!(
            message.code.as_deref(),
            Some("collector.check_unknown_no_account")
        );
        assert!(
            message.text.contains("account"),
            "the English fallback has to name it too: {}",
            message.text
        );
    }

    /// A conclusive answer leaves nothing behind, whichever way it went.
    #[test]
    fn a_conclusive_answer_carries_no_message() {
        for status in [LinkStatus::Online, LinkStatus::Offline] {
            assert!(
                provider_message(Some(&probed(status)), true).is_none(),
                "{status:?}"
            );
            assert!(
                provider_message(Some(&probed(status)), false).is_none(),
                "{status:?}"
            );
            assert!(
                direct_message(Some(&probed(status))).is_none(),
                "{status:?}"
            );
        }
    }

    /// A direct probe that timed out is not a missing HTTP client.
    ///
    /// `probe_direct` answers `Unknown` for a timeout, a refused connection or a proxy in the
    /// way, and all of those used to be stored as "No HTTP client available" — the one thing
    /// they were not.
    #[test]
    fn an_unclear_probe_is_not_reported_as_a_missing_client() {
        let no_client = direct_message(None).expect("a message");
        let unclear = direct_message(Some(&probed(LinkStatus::Unknown))).expect("a message");
        assert_eq!(no_client.code.as_deref(), Some("collector.check_no_client"));
        assert_eq!(
            unclear.code.as_deref(),
            Some("collector.check_inconclusive")
        );
        assert_ne!(no_client.text, unclear.text);
    }

    /// Every code this module hands out is distinct and shaped like the ones the REST layer
    /// uses, so none of them can silently collide in the catalogue.
    #[test]
    fn the_check_codes_are_distinct() {
        let mut codes: Vec<String> = [
            provider_message(None, true),
            provider_message(Some(&probed(LinkStatus::Unknown)), true),
            provider_message(Some(&probed(LinkStatus::Unknown)), false),
            direct_message(None),
            direct_message(Some(&probed(LinkStatus::Unknown))),
            // Both halves of the unresolvable verdict, asked directly so the list stays
            // independent of what happens to be installed while it runs.
            Some(unresolvable_message(true)),
            Some(unresolvable_message(false)),
        ]
        .into_iter()
        .map(|message| message.expect("a message").code.expect("a code"))
        .collect();
        codes.sort();
        let total = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), total, "two situations share one code");
        for code in &codes {
            assert!(code.starts_with("collector.check_"), "{code}");
        }
    }

    /// A page that is not a file says so, and says it differently from a check that reached
    /// no conclusion -- the row it produces cannot be queued, so it has to carry its reason.
    ///
    /// The hoster whose plugin *is* installed is the case `collector.check_not_a_file` was
    /// meant for, and it keeps the code: the host is supported and this particular address
    /// still serves a page (RD-120-18).
    #[test]
    fn a_page_on_a_supported_host_is_still_not_a_file() {
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        rd_provider_registry::replace_dynamic(vec![hoster_row("1fichier.com")]);
        let not_a_file =
            direct_message(Some(&probed(LinkStatus::Unresolvable))).expect("a message");
        let unclear = direct_message(Some(&probed(LinkStatus::Unknown))).expect("a message");
        assert_eq!(
            not_a_file.code.as_deref(),
            Some("collector.check_not_a_file")
        );
        assert_ne!(not_a_file.code, unclear.code);
        rd_provider_registry::replace_dynamic(Vec::new());
    }

    /// The defect RD-120-18 was raised for: the installation, not the address.
    ///
    /// Each of the four reported cases is a host no installed plugin registers. The message
    /// has to name that, because the address is not what the reader can do anything about.
    #[test]
    fn a_host_without_a_resolver_says_so_rather_than_blaming_the_address() {
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        rd_provider_registry::replace_dynamic(Vec::new());
        for address in REPORTED_HOSTS {
            let message = direct_message(Some(&unresolvable_at(address))).expect("a message");
            assert_eq!(
                message.code.as_deref(),
                Some("collector.check_no_resolver"),
                "{address}"
            );
            // The host travels as the candidate's own address, so the English fallback must
            // not try to spell it out -- that would be the assembled sentence the job forbids.
            assert!(
                !message.text.contains(
                    url::Url::parse(address)
                        .expect("URL")
                        .host_str()
                        .expect("host")
                ),
                "{address}: {}",
                message.text
            );
        }
    }

    /// Installing the plugin is the only thing that changes the verdict, and it changes it.
    ///
    /// Same address, same response, two messages -- which is the whole point: the answer is a
    /// property of the installation.
    #[test]
    fn installing_the_hoster_turns_the_message_back_into_not_a_file() {
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        let address = "https://nfile.cc/abcdef123456";
        rd_provider_registry::replace_dynamic(Vec::new());
        assert_eq!(
            direct_message(Some(&unresolvable_at(address)))
                .expect("a message")
                .code
                .as_deref(),
            Some("collector.check_no_resolver")
        );
        rd_provider_registry::replace_dynamic(vec![hoster_row("nfile.cc")]);
        assert_eq!(
            direct_message(Some(&unresolvable_at(address)))
                .expect("a message")
                .code
                .as_deref(),
            Some("collector.check_not_a_file")
        );
        rd_provider_registry::replace_dynamic(Vec::new());
    }

    /// `controlc.com` as it actually answered on 2026-09-22, replayed.
    ///
    /// The headers are the recorded ones: HTTP 200, `text/html; charset=UTF-8`, no
    /// `Content-Disposition`. That is what made `looks_downloadable` refuse the response and
    /// what produced the reported message. Running the real probe against them proves the
    /// whole path, not just the last decision: probe, verdict, message.
    #[tokio::test]
    async fn the_recorded_controlc_response_reports_the_missing_resolver() {
        let page = axum::Router::new().route(
            "/1a2b3c4d",
            axum::routing::any(|| async {
                (
                    [
                        (axum::http::header::CONTENT_TYPE, "text/html; charset=UTF-8"),
                        (axum::http::header::CACHE_CONTROL, "private, no-store"),
                    ],
                    RECORDED_CONTROLC_BODY,
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, page).await;
        });
        let probed = super::probe_direct(
            &reqwest::Client::new(),
            &[],
            format!("http://{address}/1a2b3c4d").parse().expect("URL"),
        )
        .await;
        assert_eq!(probed.status, LinkStatus::Unresolvable);
        // The probe never reads the provider table; only the message does. Taking the lock
        // here, after the last await, keeps it from being held across one.
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        rd_provider_registry::replace_dynamic(Vec::new());
        assert_eq!(
            direct_message(Some(&probed))
                .expect("a message")
                .code
                .as_deref(),
            Some("collector.check_no_resolver")
        );
    }

    /// The head of the recorded body, kept short: the judgement is made on the headers, and
    /// the bytes are here only so the response is the real one rather than an empty stub.
    const RECORDED_CONTROLC_BODY: &str = concat!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n",
        "<title>ControlC Pastebin - The easiest way to host your text</title>\n",
        "</head>\n<body>\n<div id=\"paste\"></div>\n</body>\n</html>\n"
    );

    /// An indexer link is XML a few kilobytes long, which is not downloadable content and
    /// must not be: it is imported. Once it has been re-routed, the verdict is withdrawn.
    #[test]
    fn a_reroutered_document_keeps_its_place_in_the_list() {
        let page = rerouted_document(&Rerouted::No, Some(probed(LinkStatus::Unresolvable)));
        assert_eq!(page.expect("a result").status, LinkStatus::Unresolvable);
        let nzb = rerouted_document(&Rerouted::Container, Some(probed(LinkStatus::Unresolvable)));
        assert_eq!(nzb.expect("a result").status, LinkStatus::Online);
        let gone = rerouted_document(&Rerouted::Container, Some(probed(LinkStatus::Offline)));
        assert_eq!(gone.expect("a result").status, LinkStatus::Offline);
        let unread = rerouted_document(
            &Rerouted::Torrent(None),
            Some(probed(LinkStatus::Unresolvable)),
        )
        .expect("a result");
        assert_eq!(unread.status, LinkStatus::Online);
        assert_eq!(unread.file_name, None);
    }

    /// RD-120-68: a re-routed torrent is named after its `info.name`, never after the token
    /// its address ends in.
    #[test]
    fn a_rerouted_torrent_is_named_after_its_info_name() {
        let mut bytes = b"d8:announce31:http://tracker.example/announce4:infod".to_vec();
        bytes.extend_from_slice(b"6:lengthi2048e4:name22:2026.09.16 Weekly Pack");
        bytes.extend_from_slice(b"12:piece lengthi16384e6:pieces20:");
        bytes.extend_from_slice(&[1_u8; 20]);
        bytes.extend_from_slice(b"ee");
        let torrent = rd_torrent::parse_torrent(&bytes).expect("torrent");
        let named = rerouted_document(
            &Rerouted::Torrent(Some(Box::new(torrent))),
            Some(probed(LinkStatus::Unresolvable)),
        )
        .expect("a result");
        assert_eq!(named.status, LinkStatus::Online);
        assert_eq!(named.file_name.as_deref(), Some("2026.09.16 Weekly Pack"));
        assert_eq!(named.size.map(rd_core::ByteCount::get), Some(2_048));
    }

    #[test]
    fn a_declared_manifest_type_is_always_worth_reading() {
        for kind in [
            "application/vnd.apple.mpegurl",
            "application/x-mpegurl; charset=utf-8",
            "application/dash+xml",
        ] {
            assert!(manifest_plausible(Some(kind), None), "{kind}");
        }
    }

    #[test]
    fn a_small_text_response_is_worth_reading() {
        // The case this exists for: a signed CDN address with no extension.
        assert!(manifest_plausible(
            Some("text/plain; charset=utf-8"),
            Some(4_096)
        ));
    }

    #[test]
    fn a_large_or_binary_response_is_not() {
        // Reading a slice of every video in the queue is exactly what the gate prevents.
        assert!(!manifest_plausible(Some("video/mp4"), Some(4_096)));
        assert!(!manifest_plausible(
            Some("text/plain"),
            Some(64 * 1024 * 1024)
        ));
        assert!(!manifest_plausible(Some("application/zip"), None));
        assert!(!manifest_plausible(Some("text/html"), Some(2_048)));
    }
}

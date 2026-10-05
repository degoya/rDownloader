//! One check job: the candidates it claims, and the way each of them is checked.

use super::*;

/// The claimed candidates of one job, sorted by the way each of them is checked.
struct Routes {
    by_account: HashMap<rd_core::AccountId, Vec<(LinkCandidate, bool)>>,
    direct: Vec<LinkCandidate>,
    unsupported: Vec<LinkCandidate>,
    media: Vec<LinkCandidate>,
    gallery: Vec<LinkCandidate>,
    remote: Vec<LinkCandidate>,
}

impl LinkCheckService {
    pub(super) async fn process(&self, job: CheckJob) -> Result<()> {
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
        let (claimed, guards) = self.screen_remote(claimed).await?;
        if claimed.is_empty() {
            if let Some(batch_id) = batch_id {
                self.inner
                    .database
                    .regroup_collector_batches(vec![batch_id])
                    .await?;
            }
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
        let routes = route(claimed, &accounts, &catalogues);
        self.check_by_account(routes.by_account, &accounts, &hints)
            .await;
        self.check_direct(routes.direct, &guards).await?;
        for candidate in routes.media {
            let guard = guards.get(&candidate.id).cloned();
            self.check_media(candidate, guard.as_ref()).await;
        }
        for candidate in routes.remote {
            let guard = guards.get(&candidate.id).cloned();
            self.check_remote(candidate, guard.as_ref()).await;
        }
        self.pass_unprobed(routes.gallery, &hints).await;
        self.mark_unsupported(routes.unsupported, &resolvers, &hints)
            .await;
        if let Some(batch_id) = batch_id {
            self.inner
                .database
                .regroup_collector_batches(vec![batch_id])
                .await?;
        }
        Ok(())
    }

    /// Holds the links a document or a page proposed to their address rule (RD-150-03).
    ///
    /// A candidate the person added themselves passes as it always did. One a Metalink, an
    /// intake parser or a crawler on a stranger's page proposed is checked before anything
    /// asks for it: an address that is, or resolves to, this machine — or the person's own
    /// network, unless they handed the document over themselves — is marked with
    /// `collector.check_internal_address` and never requested. The others go on with their
    /// rule, which the HTTP probes below take into a guarded client, so a name that answers
    /// differently when the connection is made is refused there too. So do the WebDAV
    /// `PROPFIND` (the same guarded client) and the FTP and SFTP probes, which resolve the host
    /// once, check every address and open their socket to exactly those. The yt-dlp probe
    /// opens its own sockets in another process: it has this check before the address is
    /// handed over and nothing after it. A bucket check goes to the endpoint of the person's
    /// own storage profile; the link names only the bucket and the key.
    async fn screen_remote(
        &self,
        claimed: Vec<LinkCandidate>,
    ) -> Result<(
        Vec<LinkCandidate>,
        HashMap<CandidateId, rd_http::AddressPolicy>,
    )> {
        let reaches = self.inner.database.candidates_remote_reach().await?;
        let mut guards = HashMap::new();
        let mut admitted = Vec::with_capacity(claimed.len());
        for candidate in claimed {
            let Some(local_network) = reaches.get(&candidate.id).copied() else {
                admitted.push(candidate);
                continue;
            };
            let policy = self.inner.scheduler.remote_address_policy(local_network);
            if let Err(rd_http::TargetRefusal::Refused(_)) =
                rd_http::check_target(&policy, &rd_http::SystemLookup, &candidate.url).await
            {
                tracing::warn!(
                    candidate_id = %candidate.id,
                    "a proposed link points at an address it may not reach; it was not checked"
                );
                let message = rd_core::CandidateMessage::coded(
                    CODE_CHECK_INTERNAL_ADDRESS,
                    "This link points at this machine or into your own network, so it was not checked",
                );
                self.record(&candidate, None, Some(message)).await;
                continue;
            }
            guards.insert(candidate.id, policy);
            admitted.push(candidate);
        }
        Ok((admitted, guards))
    }

    /// Checks every hoster link through the account it was routed to.
    async fn check_by_account(
        &self,
        by_account: HashMap<rd_core::AccountId, Vec<(LinkCandidate, bool)>>,
        accounts: &[Account],
        hints: &HashMap<CandidateId, link_check_cache::CacheHint>,
    ) {
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
    }

    /// Probes the direct links, four at a time, and reroutes the ones that turn out to be a
    /// document.
    async fn check_direct(
        &self,
        direct: Vec<LinkCandidate>,
        guards: &HashMap<CandidateId, rd_http::AddressPolicy>,
    ) -> Result<()> {
        if !direct.is_empty() {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
            let mut tasks = Vec::with_capacity(direct.len());
            for candidate in direct {
                let permit = Arc::clone(&semaphore).acquire_owned().await?;
                let scheduler = self.inner.scheduler.clone();
                let guard = guards.get(&candidate.id).cloned();
                tasks.push(tokio::spawn(async move {
                    let _permit = permit;
                    let network = match guard {
                        Some(policy) => scheduler.guarded_client(&candidate.url, policy).await,
                        None => scheduler.direct_client(&candidate.url).await,
                    };
                    let result = match network {
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
                    let guard = guards.get(&candidate.id);
                    let rerouted = self.reclassify_document(&candidate, guard).await;
                    // A manifest does not have to announce itself in its address: a signed
                    // CDN link has no extension and is often served as `text/plain`
                    // (RD-080-06). Reclassifying here is what keeps it from being queued as
                    // a text file.
                    if matches!(rerouted, Rerouted::No)
                        && result.is_some()
                        && self.classify_manifest(&candidate, false, guard).await
                    {
                        continue;
                    }
                    let result = rerouted_document(&rerouted, result);
                    let message = direct_message(result.as_ref());
                    self.record(&candidate, result, message).await;
                }
            }
        }
        Ok(())
    }

    /// Records the links that pass without a probe: galleries, torrents, NZBs and recordings.
    async fn pass_unprobed(
        &self,
        gallery: Vec<LinkCandidate>,
        hints: &HashMap<CandidateId, link_check_cache::CacheHint>,
    ) {
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
    }

    /// Marks the hoster links no account can check, with whether a free download is possible.
    async fn mark_unsupported(
        &self,
        unsupported: Vec<LinkCandidate>,
        resolvers: &rd_plugin_host::ResolverService,
        hints: &HashMap<CandidateId, link_check_cache::CacheHint>,
    ) {
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
    }
}

/// Sorts the claimed candidates by the way each of them is checked.
fn route(
    claimed: Vec<LinkCandidate>,
    accounts: &[Account],
    catalogues: &[(rd_core::AccountId, Vec<String>)],
) -> Routes {
    // The candidate travels with whether *its own* hoster has an account, because that is
    // what an inconclusive answer has to be read against: a check that ran through a
    // covering multihoster, or through no account of this hoster at all, is inconclusive
    // for a reason the reader can do something about (RD-109-43).
    let mut by_account: HashMap<rd_core::AccountId, Vec<(LinkCandidate, bool)>> = HashMap::new();
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
            rd_core::FTP_PROVIDER
                | rd_core::SFTP_PROVIDER
                | rd_core::WEBDAV_PROVIDER
                | rd_core::OBJECT_STORAGE_PROVIDER
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
    Routes {
        by_account,
        direct,
        unsupported,
        media,
        gallery,
        remote,
    }
}

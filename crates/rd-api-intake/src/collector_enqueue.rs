//! Turns a LinkGrabber package into one download package (account selection per file).

use rd_core::{Account, CandidateId, CollectorPackageId, LinkCandidate};
use rd_db::StoreErrorKind;
use rd_scheduler::{FileSpec, PackageSpec};

use crate::{ApiError, AppState};

mod nzb;
mod replay;
mod selection;
mod sources;

pub(crate) use nzb::indexer_refusal;
use nzb::*;
use replay::*;
use selection::*;
use sources::*;

/// Result of one package enqueue.
pub struct EnqueueOutcome {
    pub package: rd_core::DownloadPackage,
    /// Files enqueued without any provider account (free/direct download attempt).
    pub free_download_files: u32,
}

/// Locks the package's links, enqueues them as one package and releases the lock.
///
/// `only` narrows it to those of the package's links it names, for a LinkGrabber whose filter
/// hides the rest; they stay behind in their package.
///
/// # The cancellation boundary
///
/// Runs on a task of its own, so that a client which disconnects mid-request cannot leave a
/// package half written. Axum drops a handler's future the moment the connection goes, and
/// this operation is a sequence of separate writes: the claim, one `create_download` per file,
/// the enrichment carried onto the rows they became, the torrent selection, and the release of
/// the claim. Abandoned between any two of them it leaves a package that reads as complete and
/// is short, links locked in `resolving` forever, or a queue package with no enrichment.
///
/// The boundary belongs *here* and not one layer down, around the scheduler's write. That was
/// tried and reverted: `carry_enrichment` below is part of the same operation, and a task
/// boundary in the middle let a reader observe the package after the files were written and
/// before the fields landed. Around the whole operation there is no such window — an observer
/// sees the package either not at all or with everything that belongs to it, and the two
/// properties hold together rather than trading against each other.
///
/// What a disconnect costs now is that the enqueue still happens. That is the right answer for
/// this operation: the client asked for it, the links were already locked when the connection
/// went, and finishing is the only outcome that leaves no repair work.
pub async fn enqueue_package(
    state: &AppState,
    id: CollectorPackageId,
    start_paused: bool,
    only: Option<Vec<CandidateId>>,
) -> Result<EnqueueOutcome, ApiError> {
    let state = state.clone();
    match tokio::spawn(async move { enqueue_locked(&state, id, start_paused, only).await }).await {
        Ok(outcome) => outcome,
        // The task panicked. It cannot be cancelled — nothing holds its handle but this line —
        // so there is no other way to get here, and a panic is an internal error, not a
        // refusal the client could act on.
        Err(error) => Err(ApiError::from(anyhow::anyhow!(
            "the enqueue did not finish: {error}"
        ))),
    }
}

/// The body of [`enqueue_package`], on the far side of the cancellation boundary.
async fn enqueue_locked(
    state: &AppState,
    id: CollectorPackageId,
    start_paused: bool,
    only: Option<Vec<CandidateId>>,
) -> Result<EnqueueOutcome, ApiError> {
    let package = state
        .database
        .get_collector_package(id)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    let claimed = state
        .database
        .claim_package_for_enqueue(id, only)
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::Busy) => ApiError::conflict(
                "collector.package_busy",
                "The package is currently being checked or has already been added",
            ),
            Some(StoreErrorKind::NoEnqueueableLinks) => ApiError::conflict(
                "collector.package_no_links",
                "The package contains no downloadable links",
            ),
            _ => error.into(),
        })?;
    let restore: Vec<_> = claimed
        .iter()
        .map(|(candidate, previous)| (candidate.id, *previous))
        .collect();
    let result = build_and_enqueue(
        state,
        &package,
        claimed.into_iter().map(|(c, _)| c).collect(),
        start_paused,
    )
    .await;
    state
        .database
        .finish_package_enqueue(id, result.is_ok(), restore)
        .await?;
    if result.is_ok() {
        // The torrents the check kept for these links now belong to their rows (RD-130-18).
        crate::torrent_intake::prune_checked_torrents(state).await;
    }
    result
}

async fn build_and_enqueue(
    state: &AppState,
    package: &rd_core::CollectorPackage,
    candidates: Vec<LinkCandidate>,
    start_paused: bool,
) -> Result<EnqueueOutcome, ApiError> {
    ensure_media_selections(&candidates)?;
    let destination = crate::destination::intake_destination(
        &state.database,
        &state.scheduler,
        package.category_id,
    )
    .await?;
    let mirror_detection = crate::settings_store::stored_settings(&state.database)
        .await
        .map_or(true, |settings| settings.mirror_detection);
    let accounts: Vec<Account> = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .filter(|account| account.enabled)
        .collect();
    let mut files = Vec::with_capacity(candidates.len());
    // Packages the NZB import path created; they are real queue packages, not queue files.
    let mut imported: Vec<rd_core::DownloadPackage> = Vec::new();
    let mut free_download_files = 0u32;
    let mut torrent_states = Vec::new();
    // What the LinkGrabber worked out about each link's mirrors (RD-110-18), kept beside the
    // rows it becomes and read by position. The queue does not recompute it: the answer here
    // was built from what the site rule declared and from the names and sizes the online
    // check brought back, and it carries the member a person selected (RD-110-20).
    let mut mirrors: Vec<Option<rd_core::CandidateMirror>> = Vec::new();
    // What an enricher found about each link, kept beside the queue rows it becomes
    // (RD-107-02). Matched back by source address after the scheduler assigned the ids:
    // the candidate that carried the fields and the row that inherits them are the same
    // link, and the address is what says so.
    let mut enrichment_by_url: Vec<(url::Url, Vec<rd_core::EnrichmentField>)> = Vec::new();
    // NZB candidates become packages of their own, so they are carried separately.
    let mut imported_enrichment: Vec<(rd_core::PackageId, Vec<rd_core::EnrichmentField>)> =
        Vec::new();
    for candidate in candidates {
        // A link a document or a page proposed keeps to its address rule into the queue
        // (RD-150-03); `None` for one the person gave.
        let reach = state.database.candidate_remote_reach(candidate.id).await?;
        let account_id = account_for_candidate(state, &accounts, &candidate).await?;
        if account_id.is_none() && is_registry_hoster(&candidate) {
            free_download_files += 1;
        }
        // An NZB is imported into the Usenet queue rather than downloaded as a file
        // (RD-080-11). Handled before the FileSpec is built, because it contributes none.
        if candidate.provider.as_deref() == Some(rd_core::NZB_PROVIDER) {
            // `start_paused` travels here too: an NZB candidate used to start downloading
            // immediately although the package was enqueued paused, with no error and no
            // disabled control to show for it (RD-107-09).
            let nzb = import_nzb_candidate(
                state,
                &candidate,
                package,
                &destination,
                start_paused,
                reach,
            )
            .await?;
            if !candidate.enrichment.is_empty() {
                imported_enrichment.push((nzb.id, candidate.enrichment.clone()));
            }
            imported.push(nzb);
            continue;
        }
        let kind = download_kind(state, &candidate);
        // A reviewed remote directory becomes one queue row per selected file rather than
        // a single opaque row, so each file resumes and retries on its own.
        if let Some(expanded) = remote_files(state, &candidate, kind, reach).await? {
            // One candidate becoming many files is no mirror group, and `mirrors` is read by
            // position: an entry left behind here would be attributed to whichever link
            // happens to follow.
            mirrors.extend(std::iter::repeat_n(None, expanded.len()));
            files.extend(expanded);
            continue;
        }
        mirrors.push(candidate.mirror.clone());
        if !candidate.enrichment.is_empty() {
            enrichment_by_url.push((candidate.url.clone(), candidate.enrichment.clone()));
        }
        let (file, torrent_state) = file_spec(state, candidate, kind, account_id, reach).await?;
        torrent_states.extend(torrent_state);
        files.push(file);
    }
    group_mirrors(&mut files, &mirrors, mirror_detection);
    let password = state
        .database
        .collector_package_password(package.id)
        .await?;
    // A package whose links were all NZB imports contributes no queue files: the import
    // path created its own package rows. Calling the scheduler with an empty file list
    // would fail, and the enqueue did in fact succeed (RD-080-11).
    if files.is_empty() {
        carry_imported_enrichment(state, &imported_enrichment).await?;
        let package = imported.pop().ok_or_else(|| {
            ApiError::unprocessable("collector.package_empty", "That package has no links")
        })?;
        return Ok(EnqueueOutcome {
            package,
            free_download_files,
        });
    }
    // The reviewed file tree and selection travel with the rows they belong to, written before
    // a row can start (audit 1.9.1, API-07).
    let torrent_states = torrent_states
        .into_iter()
        .map(|(source, stored)| (source, rd_core::TorrentJobState::from_candidate(stored)))
        .collect();
    // A name the LinkGrabber derived gets the package-name rules of the package's category, as
    // its `queue_name` showed; one somebody stated or renamed stays as it is (RD-1140-05).
    let name = if package.auto_named {
        state
            .database
            .tidy_package_name(&package.name, package.category_id)
            .await?
    } else {
        package.name.clone()
    };
    let (created, _) = state
        .scheduler
        .enqueue_package_with_torrents(
            PackageSpec {
                name,
                destination,
                category_id: package.category_id,
                priority: package.priority,
                password,
                start_paused,
                postprocess_level: package.postprocess_level,
                script: package.script.clone(),
                // Written with the package row rather than behind it (RD-107-02). The second
                // write it replaces left the package readable, and empty, for the moment in
                // between — which is exactly what migration 0063 is named against.
                enrichment: union_enrichment(&enrichment_by_url),
            },
            files,
            torrent_states,
        )
        .await?;
    carry_imported_enrichment(state, &imported_enrichment).await?;
    Ok(EnqueueOutcome {
        package: created,
        free_download_files,
    })
}

/// The kind of queue row a link becomes, by the provider the check gave it.
fn download_kind(state: &AppState, candidate: &LinkCandidate) -> rd_core::DownloadKind {
    match candidate.provider.as_deref() {
        Some(rd_core::MEDIA_PROVIDER) => rd_core::DownloadKind::Media,
        // A live manifest the online check identified (RD-080-06). It goes to the
        // recorder, which is open-ended by design; the file downloader would produce a
        // job that can never reach 100 %.
        Some(rd_core::RECORD_PROVIDER) => rd_core::DownloadKind::Record,
        Some(rd_core::GALLERY_PROVIDER) => rd_core::DownloadKind::Gallery,
        Some(rd_core::TORRENT_PROVIDER) => rd_core::DownloadKind::Torrent,
        Some(rd_core::FTP_PROVIDER) => rd_core::DownloadKind::Ftp,
        Some(rd_core::SFTP_PROVIDER) => rd_core::DownloadKind::Sftp,
        Some(rd_core::OBJECT_STORAGE_PROVIDER) => rd_core::DownloadKind::ObjectStorage,
        // A scheme an installed transfer backend claims goes to the plugin runner. The
        // scheme is the whole routing rule: a backend that claims one owns every link
        // carrying it, which is why two backends cannot claim the same one.
        _ if claims_scheme(state, candidate.url.scheme()) => rd_core::DownloadKind::Plugin,
        // WebDAV files are fetched with a plain HTTP GET, so they stay Http rows and
        // reuse the existing engine's resume, auth profiles, proxy and rate limit.
        _ => rd_core::DownloadKind::Http,
    }
}

/// The queue file one link becomes, with the torrent state its check kept, if it kept one.
async fn file_spec(
    state: &AppState,
    candidate: LinkCandidate,
    kind: rd_core::DownloadKind,
    account_id: Option<rd_core::AccountId>,
    reach: Option<bool>,
) -> Result<(FileSpec, Option<(url::Url, rd_core::TorrentCandidateState)>), ApiError> {
    let file_name = candidate.file_name.clone().unwrap_or_else(|| {
        candidate
            .url
            .path_segments()
            .and_then(Iterator::last)
            .filter(|value| !value.is_empty())
            .unwrap_or("download.bin")
            .to_owned()
    });
    let media = candidate
        .media
        .as_ref()
        .and_then(rd_core::MediaInfo::selection)
        .or_else(|| record_selection(&candidate));
    // Magnet links have no path segment; derive their name from the magnet itself.
    let file_name = if kind == rd_core::DownloadKind::Torrent
        && candidate.file_name.is_none()
        && candidate.url.scheme() == "magnet"
    {
        crate::torrent_intake::magnet_name(&candidate.url)
    } else {
        file_name
    };
    let replay = replay_spec_for(state, &candidate).await?;
    // The vaulted link fragment travels with the link (RD-110-38). A reference, never
    // the key: the queue row inherits it and the candidate stops pointing at it in the
    // same transaction, so exactly one row owns the secret at any moment.
    let secret_fragment = state
        .database
        .candidate_secret_fragment_ref(candidate.id)
        .await?
        .map(|reference| rd_scheduler::SecretFragmentSpec {
            reference,
            candidate_id: Some(candidate.id),
        });
    let mut torrent_state = None;
    let mut source = candidate.url.clone();
    if candidate.torrent.is_some()
        && let Some(stored) = state.database.candidate_torrent_state(candidate.id).await?
    {
        // A torrent the check already read is queued from that copy, not fetched a
        // second time from its address (RD-130-18).
        if kind == rd_core::DownloadKind::Torrent {
            source =
                crate::torrent_intake::checked_torrent_source(state, &candidate, &stored).await;
        }
        torrent_state = Some((source.clone(), stored));
    }
    // The mirrors and hashes a Metalink parser stated for this link (RD-150-03). Only
    // for a plain HTTP row: every other kind has a runner of its own that knows no sets.
    let stated = if kind == rd_core::DownloadKind::Http {
        match state.database.candidate_source_set(candidate.id).await? {
            Some(set) => Some(set),
            None => declared_checksum(state, &candidate, &source, reach).await?,
        }
    } else {
        None
    };
    // Without a set, a proposed link's own address becomes its one source row: that row is
    // how the transfer knows the address is a stranger's and which rule it keeps to.
    let source_set = stated
        .or_else(|| reach.and_then(|local_network| held_to_reach(kind, &source, local_network)))
        .map(Box::new);
    let file = FileSpec {
        source,
        file_name,
        size: candidate.size,
        account_id,
        proxy_profile_id: None,
        // What the link was reviewed with. `Auto` still means scope matching decides;
        // an explicit choice made in the LinkGrabber has to survive into the queue, or
        // the first attempt at a private page goes out with the wrong session
        // (RD-080-04).
        auth_profile: candidate.auth_profile,
        kind,
        media,
        replay,
        // The online check already resolved which stored login reaches the server;
        // the transfer must authenticate the same way the probe did.
        remote_credential_id: candidate.remote_credential_id,
        // Filled in below, once the whole package's links are known.
        mirror_group: None,
        skipped: false,
        enrichment: candidate.enrichment.clone(),
        secret_fragment,
        source_set,
    };
    Ok((file, torrent_state))
}

/// Writes the enricher fields of every NZB candidate onto the package its import created.
///
/// Separate from the ordinary path because an NZB is not a queue file: `import_nzb_candidate`
/// builds a package of its own, so there is no row to match by address.
async fn carry_imported_enrichment(
    state: &AppState,
    imported: &[(rd_core::PackageId, Vec<rd_core::EnrichmentField>)],
) -> Result<(), ApiError> {
    for (package_id, fields) in imported {
        state
            .database
            .carry_enrichment(*package_id, fields.clone(), Vec::new())
            .await?;
    }
    Ok(())
}

/// The package's fields: every file's, once each.
///
/// Deduplicated by plugin and name rather than by value, because two links of one release
/// answer with the same field from the same plugin and a package header repeating it five
/// times says nothing more than one that says it once. The first occurrence wins, so the
/// order the links were reviewed in is the order the chips appear in.
fn union_enrichment(
    per_url: &[(url::Url, Vec<rd_core::EnrichmentField>)],
) -> Vec<rd_core::EnrichmentField> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut union = Vec::new();
    for (_, fields) in per_url {
        for field in fields {
            let key = (field.plugin_id.clone(), field.name.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            union.push(field.clone());
        }
    }
    union
}

/// `true` when the link belongs to a known hoster from the provider registry (as opposed to
/// plain `direct_http` links or the media/gallery/torrent pseudo-providers).
fn is_registry_hoster(candidate: &LinkCandidate) -> bool {
    candidate
        .provider
        .as_deref()
        .is_some_and(|provider| rd_provider_registry::by_slug(provider).is_some())
}

/// Provider account for a link: explicit route → matching provider → Premiumize catalogue.
pub async fn account_for_candidate(
    state: &AppState,
    accounts: &[Account],
    candidate: &LinkCandidate,
) -> Result<Option<rd_core::AccountId>, ApiError> {
    if let Some(id) = candidate.route.as_ref().and_then(|route| route.account_id) {
        return Ok(Some(id));
    }
    let provider = candidate.provider.as_deref().unwrap_or("direct_http");
    if matches!(
        provider,
        rd_core::MEDIA_PROVIDER | rd_core::GALLERY_PROVIDER | rd_core::TORRENT_PROVIDER
    ) {
        return Ok(None);
    }
    if let Some(account) = accounts
        .iter()
        .find(|account| account.provider.eq_ignore_ascii_case(provider))
    {
        return Ok(Some(account.id));
    }
    // No matching or covering account is not an error: the download proceeds without an
    // account as a free/direct attempt, exactly like the direct-add path in download_handlers.
    Ok(crate::hosters::fallback_account(state, accounts, &candidate.url).await)
}

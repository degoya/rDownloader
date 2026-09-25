//! Turns a LinkGrabber package into one download package (account selection per file).

use rd_core::{Account, CandidateId, CollectorPackageId, LinkCandidate};
use rd_db::StoreErrorKind;
use rd_scheduler::{FileSpec, PackageSpec};

use crate::{ApiError, AppState};

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
        crate::torrent_handlers::prune_checked_torrents(state).await;
    }
    result
}

/// The consent gate.
///
/// A captured request that would send something the browser sent — a POST, a body, or a
/// signed URL — may only be enqueued once a person has approved exactly this template. A
/// plain captured GET carries no credentials and needs no approval, so browser interception
/// behaves exactly as it did before replay existed.
async fn replay_spec_for(
    state: &AppState,
    candidate: &LinkCandidate,
) -> Result<Option<rd_scheduler::ReplaySpec>, ApiError> {
    let Some(request) = candidate.request.clone() else {
        return Ok(None);
    };
    if !rd_core::needs_consent(&request) {
        return Ok(None);
    }
    if !request.replayable {
        let mut error = ApiError::conflict(
            crate::error_codes::REPLAY_NOT_REPLAYABLE,
            "This captured request cannot be reproduced",
        );
        if let Some(reason) = request.blocked_reason {
            // Serialized through serde so the parameter matches the `snake_case` variant
            // name the web UI translates.
            let reason = serde_json::to_string(&reason).unwrap_or_default();
            error = error.with_param("reason", reason.trim_matches('"'));
        }
        return Err(error);
    }
    let consent = state
        .database
        .candidate_replay_consent(candidate.id)
        .await?
        .ok_or_else(|| {
            ApiError::conflict(
                crate::error_codes::REPLAY_CONSENT_REQUIRED,
                "This download sends credentials and needs explicit approval first",
            )
            .with_param("candidate_id", candidate.id)
        })?;
    // Consent is bound to the template it was given for: a changed method, body or origin
    // set is a different decision, so it has to be made again.
    let expected = rd_core::stable_hash(&candidate.url, &request, None);
    if consent.template_hash != expected {
        return Err(ApiError::conflict(
            crate::error_codes::REPLAY_TEMPLATE_CHANGED,
            "The captured request changed since it was approved",
        )
        .with_param("candidate_id", candidate.id));
    }
    let body_ref = state.database.candidate_body_ref(candidate.id).await?;
    Ok(Some(rd_scheduler::ReplaySpec {
        request,
        consent,
        body_ref,
        candidate_id: Some(candidate.id),
    }))
}

async fn build_and_enqueue(
    state: &AppState,
    package: &rd_core::CollectorPackage,
    candidates: Vec<LinkCandidate>,
    start_paused: bool,
) -> Result<EnqueueOutcome, ApiError> {
    ensure_media_selections(&candidates)?;
    let destination = crate::config_handlers::download_destination(state, package.category_id)
        .await?
        .unwrap_or_else(|| state.scheduler.downloads_directory().to_path_buf());
    crate::storage_capacity::ensure_intake_allowed(&state.scheduler.capacity(), &destination)
        .await?;
    let mirror_detection = crate::handlers::stored_settings(&state.database)
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
        let account_id = account_for_candidate(state, &accounts, &candidate).await?;
        if account_id.is_none() && is_registry_hoster(&candidate) {
            free_download_files += 1;
        }
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
        // An NZB is imported into the Usenet queue rather than downloaded as a file
        // (RD-080-11). Handled before the FileSpec is built, because it contributes none.
        if candidate.provider.as_deref() == Some(rd_core::NZB_PROVIDER) {
            // `start_paused` travels here too: an NZB candidate used to start downloading
            // immediately although the package was enqueued paused, with no error and no
            // disabled control to show for it (RD-107-09).
            let nzb = import_nzb_candidate(state, &candidate, package, &destination, start_paused)
                .await?;
            if !candidate.enrichment.is_empty() {
                imported_enrichment.push((nzb.id, candidate.enrichment.clone()));
            }
            imported.push(nzb);
            continue;
        }
        let kind = match candidate.provider.as_deref() {
            Some(rd_core::MEDIA_PROVIDER) => rd_core::DownloadKind::Media,
            // A live manifest the online check identified (RD-080-06). It goes to the
            // recorder, which is open-ended by design; the file downloader would produce a
            // job that can never reach 100 %.
            Some(rd_core::RECORD_PROVIDER) => rd_core::DownloadKind::Record,
            Some(rd_core::GALLERY_PROVIDER) => rd_core::DownloadKind::Gallery,
            Some(rd_core::TORRENT_PROVIDER) => rd_core::DownloadKind::Torrent,
            Some(rd_core::FTP_PROVIDER) => rd_core::DownloadKind::Ftp,
            Some(rd_core::SFTP_PROVIDER) => rd_core::DownloadKind::Sftp,
            // A scheme an installed transfer backend claims goes to the plugin runner. The
            // scheme is the whole routing rule: a backend that claims one owns every link
            // carrying it, which is why two backends cannot claim the same one.
            _ if claims_scheme(state, candidate.url.scheme()) => rd_core::DownloadKind::Plugin,
            // WebDAV files are fetched with a plain HTTP GET, so they stay Http rows and
            // reuse the existing engine's resume, auth profiles, proxy and rate limit.
            _ => rd_core::DownloadKind::Http,
        };
        // A reviewed remote directory becomes one queue row per selected file rather than
        // a single opaque row, so each file resumes and retries on its own.
        if let Some(expanded) = remote_files(state, &candidate, kind).await? {
            // One candidate becoming many files is no mirror group, and `mirrors` is read by
            // position: an entry left behind here would be attributed to whichever link
            // happens to follow.
            mirrors.extend(std::iter::repeat_n(None, expanded.len()));
            files.extend(expanded);
            continue;
        }
        // Magnet links have no path segment; derive their name from the magnet itself.
        let file_name = if kind == rd_core::DownloadKind::Torrent
            && candidate.file_name.is_none()
            && candidate.url.scheme() == "magnet"
        {
            crate::torrent_handlers::magnet_name(&candidate.url)
        } else {
            file_name
        };
        mirrors.push(candidate.mirror.clone());
        if !candidate.enrichment.is_empty() {
            enrichment_by_url.push((candidate.url.clone(), candidate.enrichment.clone()));
        }
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
        let mut source = candidate.url.clone();
        if candidate.torrent.is_some()
            && let Some(stored) = state.database.candidate_torrent_state(candidate.id).await?
        {
            // A torrent the check already read is queued from that copy, not fetched a
            // second time from its address (RD-130-18).
            if kind == rd_core::DownloadKind::Torrent {
                source =
                    crate::torrent_handlers::checked_torrent_source(state, &candidate, &stored)
                        .await;
            }
            torrent_states.push((source.clone(), stored));
        }
        files.push(FileSpec {
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
        });
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
    let (created, created_files) = state
        .scheduler
        .enqueue_package(
            PackageSpec {
                name: package.name.clone(),
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
        )
        .await?;
    carry_imported_enrichment(state, &imported_enrichment).await?;
    // Carry the reviewed file tree and selection from the candidate to its queue row.
    for (source, stored) in torrent_states {
        if let Some(file) = created_files.iter().find(|file| file.source == source) {
            state
                .database
                .set_download_torrent_state(
                    file.id,
                    rd_core::TorrentJobState::from_candidate(stored),
                )
                .await?;
        }
    }
    Ok(EnqueueOutcome {
        package: created,
        free_download_files,
    })
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
    Ok(crate::hosters::fallback_account(state, &candidate.url).await)
}

/// Expands a reviewed remote directory into one [`FileSpec`] per selected file.
///
/// Returns `None` for anything that is not a directory listing, so the caller falls back
/// to its ordinary one-candidate-one-file path. A single-file link also returns `None`:
/// there is nothing to expand, and keeping it on the normal path preserves the file name
/// the user may have edited.
async fn remote_files(
    state: &AppState,
    candidate: &LinkCandidate,
    kind: rd_core::DownloadKind,
) -> Result<Option<Vec<FileSpec>>, ApiError> {
    let Some(stored) = state.database.candidate_listing(candidate.id).await? else {
        return Ok(None);
    };
    if stored.listing.single_file {
        return Ok(None);
    }
    let Some(base) = rd_core::RemoteTarget::parse(&candidate.url).and_then(|target| {
        // The queue row addresses the file itself, so the base is the collection URL the
        // listing was taken from.
        target.sanitized_url()
    }) else {
        return Ok(None);
    };
    let resolved = stored.resolve();
    let mut files = Vec::with_capacity(resolved.selected_files);
    for entry in resolved
        .entries
        .iter()
        .filter(|entry| !entry.is_dir && entry.included)
    {
        let Some(source) = rd_webdav::entry_url(&base, &entry.path) else {
            continue;
        };
        let file_name = entry
            .path
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or("download.bin")
            .to_owned();
        files.push(FileSpec {
            source,
            file_name,
            size: entry.size,
            account_id: None,
            proxy_profile_id: None,
            // Every file of a remote listing inherits the candidate's choice.
            auth_profile: candidate.auth_profile,
            kind,
            media: None,
            replay: None,
            remote_credential_id: candidate.remote_credential_id,
            // Files of one remote listing are members of that listing, never alternatives
            // to each other.
            mirror_group: None,
            skipped: false,
            // Every file of the listing inherits what was found about the candidate.
            enrichment: candidate.enrichment.clone(),
            secret_fragment: None,
        });
    }
    Ok(Some(files))
}

/// Whether an installed transfer backend claims this URL scheme.
fn claims_scheme(state: &AppState, scheme: &str) -> bool {
    state
        .plugin_transfer_schemes
        .iter()
        .any(|claimed| claimed.eq_ignore_ascii_case(scheme))
}

/// The selection a recorded live manifest is queued with (RD-080-06).
///
/// The stream runner reads `format` as a *streamlink quality name*, not as an extractor
/// expression — the same contract the channel monitor uses. A direct manifest has no
/// qualities to choose from before it is opened, so `best` stands in and streamlink picks.
/// Refuses a media link that carries no variant selection (RD-120-50).
///
/// Such a row can only fail: the runner's first step is `media.selection_missing`. Queued
/// anyway it looked like an ordinary download that broke later, while the reason — the page
/// offered nothing this installation can download, or the check never got that far — was
/// visible only here. Checked before anything is written, so a refused package leaves no
/// half-imported NZB behind it; the claim is released by the caller as for any refusal.
fn ensure_media_selections(candidates: &[LinkCandidate]) -> Result<(), ApiError> {
    let unselected = candidates.iter().find(|candidate| {
        candidate.provider.as_deref() == Some(rd_core::MEDIA_PROVIDER)
            && candidate
                .media
                .as_ref()
                .and_then(rd_core::MediaInfo::selection)
                .is_none()
    });
    match unselected {
        Some(candidate) => Err(ApiError::unprocessable(
            crate::error_codes::MEDIA_SELECTION_MISSING,
            "This media link has no format selection that can be downloaded",
        )
        .with_param("candidate_id", candidate.id)
        .with_param("url", rd_core::redact_url(&candidate.url))),
        None => Ok(()),
    }
}

fn record_selection(candidate: &rd_core::LinkCandidate) -> Option<rd_core::MediaSelection> {
    if candidate.provider.as_deref() != Some(rd_core::RECORD_PROVIDER) {
        return None;
    }
    Some(rd_core::MediaSelection {
        page_url: candidate.url.clone(),
        variant_id: "best".to_owned(),
        format: "best".to_owned(),
        kind: rd_core::MediaKind::Video,
        ext: "ts".to_owned(),
        title: candidate.file_name.clone().unwrap_or_default(),
        contract_version: rd_core::MEDIA_CONTRACT_VERSION,
        criteria: None,
        resolved: None,
    })
}

/// Fetches an NZB link and imports it into the Usenet queue (RD-080-11).
///
/// Deliberately the same `add_nzb_import` + `enqueue_nzb_import` path a dropped file or an
/// upload takes, so segment scheduling, PAR2 handling and post-processing behave identically
/// however the NZB arrived. The alternative — letting the ordinary HTTP engine save the
/// document into the download folder — produces a `.nzb` file on disk and no download, which
/// is what happened before this existed.
///
/// This is also what makes an NZBHydra or Prowlarr link work: those return a redirect to the
/// real indexer, and following it here is the "send the link to the downloader" behaviour
/// those proxies ask for rather than having them proxy the bytes themselves.
async fn import_nzb_candidate(
    state: &AppState,
    candidate: &LinkCandidate,
    package: &rd_core::CollectorPackage,
    destination: &std::path::Path,
    start_paused: bool,
) -> Result<rd_core::DownloadPackage, ApiError> {
    let network = state
        .scheduler
        .direct_client(&candidate.url)
        .await
        .map_err(|error| {
            ApiError::bad_gateway("collector.nzb_client_unavailable", error.to_string())
        })?;
    let fetched = rd_http::fetch_document(
        &network.client,
        candidate.url.clone(),
        &network.headers,
        rd_collector::MAX_NZB_BYTES,
    )
    .await
    .map_err(|error| {
        // The address can carry an indexer API key, so it never reaches the message.
        ApiError::bad_gateway(
            "collector.nzb_fetch_failed",
            format!("{error} ({})", rd_core::redact_url(&candidate.url)),
        )
    })?;
    // An indexer refuses inside a `200 OK`: the API limit is reached, the key is wrong, the
    // release is gone. Without this the body fails to parse and the user is told the NZB is
    // invalid, which sends them looking in the wrong place.
    if let Some(refusal) = indexer_refusal(&fetched) {
        return Err(ApiError::bad_gateway("collector.nzb_rejected", refusal));
    }
    let document = rd_collector::parse_nzb(&fetched.bytes)
        .map_err(|error| ApiError::unprocessable("collector.nzb_invalid", error.to_string()))?;
    let name = declared_nzb_name(candidate, &fetched).unwrap_or_else(|| package.name.clone());
    let (name, marker_password) = rd_files::strip_password_marker(name.trim_end_matches(".nzb"));
    // This branch used to read the file-name marker and nothing else, so a password the
    // package already held -- announced by a subscription, a DLC container or the API --
    // was silently dropped for exactly the Usenet hits that most often need one. The marker
    // still wins: it was written onto this very file, while the package's password may have
    // been meant for a sibling link.
    let package_password = state
        .database
        .collector_package_password(package.id)
        .await?;
    let password = marker_password
        .or(package_password)
        .or_else(|| document.password.clone());
    let files: Vec<rd_db::NewNzbFile> = document
        .files
        .into_iter()
        .map(|file| rd_db::NewNzbFile {
            subject: file.subject,
            poster: file.poster,
            groups: file.groups,
            segments: file
                .segments
                .into_iter()
                .map(|segment| rd_db::NewNzbSegment {
                    number: segment.number,
                    bytes: segment.bytes,
                    message_id: segment.message_id,
                })
                .collect(),
        })
        .collect();
    if files.is_empty() {
        return Err(ApiError::unprocessable(
            "collector.nzb_empty",
            "That NZB contains no files",
        ));
    }
    let import = state
        .database
        .add_nzb_import(rd_db::NewNzbImport {
            name: rd_files::sanitize_file_name(&name),
            sha256: hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&fetched.bytes)),
            category_id: package.category_id,
            // The package's category was already decided when the link entered the
            // LinkGrabber; this only labels the intake for the rare package without one.
            source: rd_core::IngressSource::Nzb,
            priority: Some(package.priority),
            import_mode: rd_core::ImportMode::Enqueue,
            // The address can carry an indexer API key; only the redacted form is stored.
            source_path: Some(rd_core::redact_url(&candidate.url)),
            password,
            // The links were announced when they entered the LinkGrabber; the files inside
            // this NZB are not a second arrival.
            announce_arrival: false,
            files,
        })
        .await?;
    Ok(state
        .database
        .enqueue_nzb_import(
            import.id,
            destination.to_path_buf(),
            package.priority,
            start_paused,
        )
        .await?)
}

/// The release name for an NZB, in the order the sources deserve to be trusted.
///
/// The feed item's title first: it is what the subscription reviewed and what the user saw
/// in the LinkGrabber, so the queue must not call the job something else. Then the two
/// headers an indexer answers with — `X-DNZB-Name` is the release, `Content-Disposition` the
/// file it would have saved as — both of which SABnzbd reads for the same reason. The
/// address's own last segment comes last: it is `api` for every hit of an indexer.
fn declared_nzb_name(
    candidate: &LinkCandidate,
    fetched: &rd_http::FetchedDocument,
) -> Option<String> {
    let clean = |value: &str| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    };
    candidate
        .file_name_declared
        .then_some(candidate.file_name.as_deref())
        .flatten()
        .and_then(clean)
        .or_else(|| fetched.header("x-dnzb-name").and_then(clean))
        .or_else(|| {
            fetched
                .header("content-disposition")
                .and_then(crate::link_check_probe::disposition_file_name)
                .as_deref()
                .and_then(clean)
        })
        .or_else(|| candidate.file_name.as_deref().and_then(clean))
}

/// The indexer's own refusal, when it answered `200 OK` with something that is not an NZB.
///
/// The `X-DNZB-*` headers SABnzbd established: `X-DNZB-Failure` states the reason outright,
/// and an `X-DNZB-RCode` other than 200 carries it in `X-DNZB-RText` ("Request limit
/// reached"). Both are worth more than the parse error the body would produce.
fn indexer_refusal(fetched: &rd_http::FetchedDocument) -> Option<String> {
    if let Some(failure) = fetched
        .header("x-dnzb-failure")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(failure.to_owned());
    }
    let code = fetched.header("x-dnzb-rcode")?.trim();
    if code == "200" {
        return None;
    }
    let text = fetched
        .header("x-dnzb-rtext")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("the indexer refused the download");
    Some(format!("{text} (code {code})"))
}

/// Carries the LinkGrabber's mirror groups into the queue (RD-110-20).
///
/// The queue used to work a group out for itself at this point (RD-094-05) from the declared
/// file names and sizes. That was a second answer to a question the LinkGrabber had already
/// answered better: its group (RD-110-18) knows what a site rule declared, was recomputed
/// once the online check brought the real names and sizes, and holds the member a person
/// picked. Two notions of the same thing can disagree, and the weaker one won here because it
/// ran last. So nothing is computed any more — the answer is carried.
///
/// Two things are still decided here, because they are the queue's and not the LinkGrabber's:
/// a transport that has no alternative routes is never grouped, and the selected member is
/// moved into the group's first slot so the queue can read the group's verdict off it when
/// every mirror has failed.
fn group_mirrors(
    files: &mut [rd_scheduler::FileSpec],
    mirrors: &[Option<rd_core::CandidateMirror>],
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let mut groups: std::collections::BTreeMap<&str, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (index, entry) in mirrors.iter().enumerate() {
        let Some(entry) = entry else { continue };
        // A Usenet or torrent row is one member of a single download, not another way to it.
        if !rd_scheduler::mirrors::groups_mirrors(files[index].kind) {
            continue;
        }
        groups.entry(entry.group.as_str()).or_default().push(index);
    }
    for (key, members) in groups {
        // A link whose mirrors were all filtered out on the way here is a download again.
        if members.len() < 2 {
            continue;
        }
        let selected = members
            .iter()
            .copied()
            .find(|index| mirrors[*index].as_ref().is_some_and(|entry| entry.selected))
            .unwrap_or(members[0]);
        for index in members.iter().copied() {
            files[index].mirror_group = Some(key.to_owned());
            files[index].skipped = index != selected;
        }
        // The queue identifies the member a group started with by position, so the chosen one
        // has to hold the group's first slot. Swapping stays inside the group's own indices,
        // which leaves every other group's positions exactly where they were.
        files.swap(selected, members[0]);
    }
}

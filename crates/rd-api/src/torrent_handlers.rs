//! Torrent endpoints: `.torrent` intake and seeding control.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_scheduler::{FileSpec, PackageSpec};
use url::Url;

use crate::{
    ApiError, AppState,
    dto::{CollectorIntakeResponse, MessageResponse},
};

/// Enqueues one torrent as a single-row package (shared by import and magnet links).
pub async fn enqueue_torrent(
    state: &AppState,
    source: Url,
    name: String,
    size: Option<u64>,
    category_id: Option<rd_core::CategoryId>,
    priority: rd_core::DownloadPriority,
) -> Result<rd_core::DownloadPackage, ApiError> {
    enqueue_torrent_with(
        &state.database,
        &state.scheduler,
        source,
        name,
        size,
        category_id,
        priority,
    )
    .await
}

/// Enqueues a torrent using explicit service components. Hotfolders use this in direct-enqueue
/// mode, where a complete [`AppState`] is deliberately not available.
pub(crate) async fn enqueue_torrent_with(
    database: &rd_db::Database,
    scheduler: &rd_scheduler::SchedulerHandle,
    source: Url,
    name: String,
    size: Option<u64>,
    category_id: Option<rd_core::CategoryId>,
    priority: rd_core::DownloadPriority,
) -> Result<rd_core::DownloadPackage, ApiError> {
    let destination = crate::destination::resolve_destination(database, category_id)
        .await?
        .unwrap_or_else(|| scheduler.downloads_directory().to_path_buf());
    crate::storage_capacity::ensure_intake_allowed(&scheduler.capacity(), &destination).await?;
    let clean = rd_files::sanitize_file_name(&name);
    let (package, _) = scheduler
        .enqueue_package(
            PackageSpec {
                name: clean.clone(),
                destination,
                category_id,
                priority,
                password: None,
                start_paused: false,
                postprocess_level: None,
                script: None,
                // Nothing looked at this: it is started from what the person chose.
                enrichment: Vec::new(),
            },
            vec![FileSpec {
                source,
                file_name: clean,
                size: size.and_then(|value| rd_core::ByteCount::new(value).ok()),
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                kind: rd_core::DownloadKind::Torrent,
                media: None,
                replay: None,
                remote_credential_id: None,
                // A torrent is fetched from its swarm; another link is a different torrent.
                mirror_group: None,
                skipped: false,
                // Nothing looked at this: it is started from what the person chose.
                enrichment: Vec::new(),
                secret_fragment: None,
            }],
        )
        .await?;
    Ok(package)
}

/// Stores parsed torrent metadata as one immediately-reviewable LinkGrabber package.
// Pre-existing, unrelated to this branch: each parameter is an independently optional piece of
// caller-supplied metadata (source label, package name, category, priority, ...), not a natural
// struct — a scoped allow is preferred here over a speculative refactor.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn add_torrent_to_collector(
    database: &rd_db::Database,
    torrent: &rd_torrent::TorrentService,
    content: &[u8],
    source: rd_core::IngressSource,
    source_label: Option<String>,
    package_name: Option<String>,
    category_id: Option<rd_core::CategoryId>,
    priority: Option<rd_core::DownloadPriority>,
) -> anyhow::Result<(
    rd_core::CollectorBatch,
    Vec<rd_core::CollectorPackage>,
    Vec<rd_core::LinkCandidate>,
)> {
    let (parsed, stored) = torrent.store_torrent_file(content).await?;
    let source_url = Url::from_file_path(&stored).map_err(|()| {
        anyhow::anyhow!("stored torrent path is not absolute: {}", stored.display())
    })?;
    let display_name = rd_files::sanitize_file_name(&parsed.name);
    let package_name = package_name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| display_name.clone());
    let intake = database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source,
            source_label,
            package_name: Some(package_name),
            password: None,
            passwords: Vec::new(),
            category_id,
            priority,
            urls: vec![source_url],
            providers: vec![Some(rd_core::TORRENT_PROVIDER.to_owned())],
            file_names: vec![Some(display_name)],
            sizes: vec![Some(
                rd_core::ByteCount::new(parsed.total_bytes).map_err(anyhow::Error::msg)?,
            )],
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await?;
    // The file tree is stored on the candidate so it can be reviewed before queueing.
    let state = rd_core::TorrentCandidateState::ready(parsed.metadata);
    for candidate in &intake.2 {
        database
            .set_candidate_torrent_state(candidate.id, state.clone())
            .await?;
    }
    Ok(intake)
}

#[utoipa::path(post, path = "/api/v1/torrents/import", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (crate::container_upload::ContainerUpload = "application/json"))), responses((status = 201, body = CollectorIntakeResponse), (status = 400, description = "The torrent is invalid or over 16 MiB, the service is off, a field is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit")))]
pub async fn import_torrent(
    State(state): State<AppState>,
    body: crate::container_upload::UploadBody,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    // A `.torrent` upload does not go through the LinkGrabber's intake, so the switch has to
    // be honoured here as well; otherwise the one path that bypasses it stays open.
    ensure_torrent_service_enabled(&state).await?;
    let upload = body.read().await?;
    let category_id = match upload.category_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(id) => Some(id.parse::<rd_core::CategoryId>().map_err(|_| {
            ApiError::bad_request("torrent.category_invalid", "Category id is not valid")
        })?),
    };
    let priority = match upload.priority.as_deref().map(str::trim) {
        None => None,
        Some("low") => Some(rd_core::DownloadPriority::Low),
        Some("normal") => Some(rd_core::DownloadPriority::Normal),
        Some("high") => Some(rd_core::DownloadPriority::High),
        Some(_) => {
            return Err(ApiError::bad_request(
                "torrent.priority_invalid",
                "Torrent priority is not valid",
            ));
        }
    };
    let package_name = upload.name;
    let Some(file) = upload.file else {
        return Err(ApiError::bad_request(
            "request.multipart_missing_file",
            "Multipart field 'file' is missing",
        ));
    };
    if file.bytes.len() > rd_torrent::MAX_TORRENT_BYTES {
        return Err(ApiError::bad_request(
            "torrent.file_too_large",
            "Torrent file exceeds the 16 MiB limit",
        ));
    }
    let source_label = file.file_name;
    let content = file.bytes;
    rd_torrent::parse_torrent(&content)
        .map_err(|error| ApiError::bad_request("torrent.file_invalid", format!("{error:#}")))?;
    let (batch, packages, candidates) = add_torrent_to_collector(
        &state.database,
        &state.torrent,
        &content,
        rd_core::IngressSource::Manual,
        source_label,
        package_name,
        category_id,
        priority,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CollectorIntakeResponse {
            batch,
            packages,
            candidates,
            skipped_excluded: 0,
            skipped_disabled: 0,
            // A torrent is handed over as a file or a magnet; no crawler is asked.
            crawled_found: 0,
            crawled_dropped: 0,
        }),
    ))
}

/// Refuses torrent work while the service is switched off.
pub(crate) async fn ensure_torrent_service_enabled(state: &AppState) -> Result<(), ApiError> {
    let settings = crate::handlers::read_settings(state).await?;
    if settings.torrent_service_enabled {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "torrent.service_disabled",
        "The BitTorrent service is switched off",
    ))
}

/// What the embedded torrent engine supports.
///
/// The UI reads this once and disables the controls the engine cannot honour, instead of
/// offering switches that would be silently ignored.
#[utoipa::path(
    get,
    path = "/api/v1/torrents/capabilities",
    tag = "downloads",
    responses((status = 200, body = rd_core::TorrentEngineCapabilities))
)]
pub async fn torrent_capabilities(
    State(state): State<AppState>,
) -> Json<rd_core::TorrentEngineCapabilities> {
    Json(state.torrent.capabilities())
}

/// Network interfaces the torrent engine can bind to.
#[utoipa::path(
    get,
    path = "/api/v1/torrents/network/interfaces",
    tag = "downloads",
    responses((status = 200, body = Vec<rd_torrent::NetworkInterface>))
)]
pub async fn torrent_interfaces() -> Json<Vec<rd_torrent::NetworkInterface>> {
    Json(rd_torrent::interfaces())
}

/// What the torrent network layer is currently doing.
#[utoipa::path(
    get,
    path = "/api/v1/torrents/network/status",
    tag = "downloads",
    responses((status = 200, body = rd_torrent::TorrentNetworkStatus))
)]
pub async fn torrent_network_status(
    State(state): State<AppState>,
) -> Json<rd_torrent::TorrentNetworkStatus> {
    Json(state.torrent.network_status().await)
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/seeding/stop", tag = "downloads", params(("id" = rd_core::DownloadId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn stop_seeding(
    State(state): State<AppState>,
    Path(id): Path<rd_core::DownloadId>,
) -> Result<Json<MessageResponse>, ApiError> {
    if state.torrent.stop_seeding(id).await? {
        Ok(Json(MessageResponse::new(
            "torrent.seeding_stopped",
            "Seeding stopped; the download is complete",
        )))
    } else {
        Err(ApiError::not_found(
            "torrent.not_seeding",
            "This download is not seeding",
        ))
    }
}

/// The source a torrent the online check re-routed is queued with (RD-130-18).
///
/// The check read the file to name the package (RD-120-68), so the address has been asked
/// once already. Queued with it, the engine would ask again, and an indexer that counts or
/// limits grabs counts two. The copy the check kept is stored like an upload instead and the
/// row gets its `file://` source. When there is no usable copy — expired, or the metadata
/// came from elsewhere — the address stays the source and the download fetches it again,
/// which is logged rather than left to happen silently.
pub(crate) async fn checked_torrent_source(
    state: &AppState,
    candidate: &rd_core::LinkCandidate,
    stored: &rd_core::TorrentCandidateState,
) -> Url {
    let Some(metadata) = stored
        .metadata
        .as_ref()
        .filter(|_| matches!(candidate.url.scheme(), "http" | "https"))
    else {
        return candidate.url.clone();
    };
    let url = rd_core::redact_url(&candidate.url);
    match state
        .torrent
        .promote_prefetched(candidate.id, &metadata.info_hash)
        .await
    {
        Ok(Some(path)) => match Url::from_file_path(&path) {
            Ok(source) => return source,
            Err(()) => {
                tracing::warn!(path = %path.display(), "stored torrent path is not absolute");
            }
        },
        Ok(None) => tracing::info!(
            %url,
            "no kept copy of the torrent read at the check; the download fetches it again"
        ),
        Err(error) => tracing::warn!(
            %url,
            %error,
            "the torrent read at the check could not be reused; the download fetches it again"
        ),
    }
    candidate.url.clone()
}

/// Drops the torrents the check kept for links that are gone or queued (RD-130-18).
///
/// Called where links leave the LinkGrabber. A path that is missed only leaves a file behind
/// until its age runs out.
pub(crate) async fn prune_checked_torrents(state: &AppState) {
    match state.database.list_candidates().await {
        Ok(candidates) => {
            let open = candidates
                .into_iter()
                .filter(|candidate| candidate.state != rd_core::LinkCandidateState::Enqueued)
                .map(|candidate| candidate.id)
                .collect();
            state.torrent.prune_prefetched(&open).await;
        }
        Err(error) => tracing::warn!(%error, "kept torrents could not be pruned"),
    }
}

/// Display name of a magnet link (its `dn` parameter, else the info hash tail).
pub fn magnet_name(url: &Url) -> String {
    url.query_pairs()
        .find(|(key, _)| key == "dn")
        .map(|(_, value)| value.into_owned())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            url.query_pairs()
                .find(|(key, _)| key == "xt")
                .map(|(_, value)| {
                    let value = value.into_owned();
                    let tail = value.rsplit(':').next().unwrap_or(&value).to_owned();
                    format!("torrent-{}", tail.chars().take(12).collect::<String>())
                })
                .unwrap_or_else(|| "torrent".to_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::{add_torrent_to_collector, magnet_name};

    const SINGLE_FILE_TORRENT: &[u8] =
        b"d4:infod6:lengthi1e4:name8:test.bin12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";

    #[test]
    fn magnet_names_come_from_dn_or_hash() {
        let named = url::Url::parse(
            "magnet:?xt=urn:btih:aaaabbbbccccddddeeeeffff0000111122223333&dn=My+ISO",
        )
        .expect("magnet");
        assert_eq!(magnet_name(&named), "My ISO");
        let unnamed =
            url::Url::parse("magnet:?xt=urn:btih:aaaabbbbccccddddeeeeffff0000111122223333")
                .expect("magnet");
        assert_eq!(magnet_name(&unnamed), "torrent-aaaabbbbcccc");
    }

    #[tokio::test]
    async fn uploaded_torrent_uses_an_absolute_source_and_waits_in_the_linkgrabber() {
        // The service defaults are relative (`data/rdownloader.sqlite3`), which triggered the
        // original "Stored torrent path is not absolute" failure.
        let directory = tempfile::tempdir_in(".").expect("relative temporary directory");
        let current = std::env::current_dir().expect("current directory");
        let relative = directory
            .path()
            .strip_prefix(current)
            .expect("temporary directory below current directory");
        assert!(!relative.is_absolute());
        let database = rd_db::Database::open(relative.join("rdownloader.sqlite3"))
            .await
            .expect("database");
        let settings = rd_torrent::shared_settings(&database)
            .await
            .expect("torrent settings");
        let torrent = rd_torrent::TorrentService::start(
            database.clone(),
            settings,
            relative.join("data"),
            relative.join("downloads"),
        );

        let (_, packages, candidates) = add_torrent_to_collector(
            &database,
            &torrent,
            SINGLE_FILE_TORRENT,
            rd_core::IngressSource::Manual,
            Some("test.torrent".to_owned()),
            Some("Reviewed torrent".to_owned()),
            None,
            Some(rd_core::DownloadPriority::High),
        )
        .await
        .expect("torrent intake");

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "Reviewed torrent");
        assert_eq!(packages[0].priority, rd_core::DownloadPriority::High);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].state, rd_core::LinkCandidateState::Online);
        assert_eq!(
            candidates[0].provider.as_deref(),
            Some(rd_core::TORRENT_PROVIDER)
        );
        assert_eq!(candidates[0].file_name.as_deref(), Some("test.bin"));
        assert_eq!(candidates[0].size.map(rd_core::ByteCount::get), Some(1));
        let stored = candidates[0]
            .url
            .to_file_path()
            .expect("file URL points at stored torrent");
        assert!(stored.is_absolute());
        assert!(stored.exists());
        torrent.shutdown();
    }
}

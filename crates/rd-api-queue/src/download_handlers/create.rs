//! A direct download request: validated and queued as a package of its own.

use rd_api_core::idempotency::IdempotencyKey;
use tracing::Instrument;

use super::*;
use crate::torrent_intake::TorrentName;

#[utoipa::path(
    post,
    path = "/api/v1/downloads",
    tag = "downloads",
    params(
        (
            "Idempotency-Key" = Option<String>,
            Header,
            description = "1 to 128 visible ASCII characters the client chooses, e.g. a UUID. A repeat with the same key and the same body is answered the download the first request created, in its state by then, instead of creating another one. Kept per credential for an hour and not across a restart; the same key with another body is refused as `request.idempotency_key_reused`"
        )
    ),
    request_body = CreateDownloadRequest,
    responses(
        (status = 201, body = rd_core::DownloadFile),
        (status = 400),
        (status = 404, description = "The download this `Idempotency-Key` created was removed since"),
        (status = 422, description = "The `Idempotency-Key` was used for a different request")
    )
)]
pub async fn create_download(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    headers: HeaderMap,
    Json(request): Json<CreateDownloadRequest>,
) -> Result<(StatusCode, Json<rd_core::DownloadFile>), ApiError> {
    let key = IdempotencyKey::from_headers(&audit.actor, &headers)?;
    let file = create_download_once(&state, key, request, None).await?;
    Ok((StatusCode::CREATED, Json(file)))
}

/// [`create_download_as`], once per `key` (RD-1480-10): a repeat of the request is answered the
/// download the first one created, and one that arrives while the first still runs waits for it.
///
/// With a key the work runs in a task of its own. A client that gives up and repeats the request
/// is the case the key exists for, and giving up drops the handler: without the task the download
/// could be written and its id never recorded, and the repeat would write a second one.
pub async fn create_download_once(
    state: &AppState,
    key: Option<IdempotencyKey>,
    request: CreateDownloadRequest,
    address_reach: Option<bool>,
) -> Result<rd_core::DownloadFile, ApiError> {
    let Some(key) = key else {
        return create_download_as(state, request, address_reach).await;
    };
    let content = serde_json::to_vec(&(&request, address_reach)).map_err(anyhow::Error::new)?;
    let outcome = state
        .queue
        .idempotency
        .claim(key, &rd_api_core::input_checks::sha256_hex(content))?;
    let state = state.clone();
    let work = async move {
        let mut created = outcome.lock_owned().await;
        if let Some(id) = *created {
            return state
                .database
                .get_download(id)
                .await?
                .ok_or_else(crate::error_codes::download_not_found);
        }
        let file = create_download_as(&state, request, address_reach).await?;
        *created = Some(file.id);
        Ok::<_, ApiError>(file)
    };
    tokio::spawn(work.in_current_span())
        .await
        .map_err(anyhow::Error::new)?
}

pub async fn create_download_inner(
    state: &AppState,
    request: CreateDownloadRequest,
) -> Result<rd_core::DownloadFile, ApiError> {
    create_download_as(state, request, None).await
}

/// Creates a direct download. `address_reach` is `None` for the person's own request; `Some(false)`
/// for one whose address somebody other than the person chose — an agent through `add_downloads`
/// (RD-1190-18): it keeps to the address rule a stranger's link keeps to (RD-150-03), so a literal
/// address on this machine or in the person's network is refused here with
/// `mirror.internal_address`, and every other one is written with its address as its one source
/// row, so the transfer refuses a name that resolves there too. The REST handler and the MCP tool
/// both reach this through [`create_download_once`], which `mcp_handler_tests` holds.
pub async fn create_download_as(
    state: &AppState,
    request: CreateDownloadRequest,
    address_reach: Option<bool>,
) -> Result<rd_core::DownloadFile, ApiError> {
    let url = Url::parse(&request.url)
        .map(rd_collector::canonical_url)
        .map_err(|error| ApiError::bad_request("download.url_invalid", error.to_string()))?;
    if url.scheme() == "magnet" {
        // A name the request stated stays; the magnet's own is tidied (RD-1140-05).
        let name = request
            .package_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map_or_else(
                || TorrentName::Derived(crate::torrent_intake::magnet_name(&url)),
                |name| TorrentName::Stated(name.to_owned()),
            );
        let package = crate::torrent_intake::enqueue_torrent(
            state,
            url,
            name,
            None,
            request.category_id,
            request.priority.unwrap_or_default(),
            request.paused,
        )
        .await?;
        // A magnet is a single-row package: its one file is the download created here.
        let file = state
            .database
            .downloads_for_package(package.id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("torrent download row missing"))?;
        return Ok(file);
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(
            "download.url_scheme_unsupported",
            "Only HTTP(S) or magnet URLs are supported for direct downloads",
        ));
    }
    if let Some(local_network) = address_reach
        && state
            .scheduler
            .remote_address_policy(local_network)
            .hop_refusal(&url)
            .is_some()
    {
        return Err(ApiError::bad_request(
            rd_core::CODE_INTERNAL_ADDRESS,
            "The address points at this machine or into the local network",
        ));
    }
    // Decoded, so `Big%20Buck.mkv` is saved as `Big Buck.mkv` (RD-1240-33).
    let inferred = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|value| !value.is_empty())
        .map_or_else(
            || rd_files::FALLBACK_FILE_NAME.to_owned(),
            rd_files::decode_path_segment,
        );
    // A URL-derived package name doubles as the folder name; extensions are stripped, and the
    // package-name rules of its category apply. A name the request stated stays (RD-1140-05).
    let package_name = match request.package_name {
        Some(name) => name,
        None => {
            state
                .database
                .tidy_package_name(
                    &rd_files::package_name_from_file_name(&inferred),
                    request.category_id,
                )
                .await?
        }
    };
    let file_name = request.file_name.unwrap_or(inferred);
    validate_network_selection(state, request.account_id, request.proxy_profile_id).await?;
    let account_id = match request.account_id {
        Some(id) => Some(id),
        None => match state.database.list_accounts().await {
            Ok(accounts) => crate::hosters::fallback_account(state, &accounts, &url).await,
            Err(_) => None,
        },
    };
    let destination = crate::destination::download_destination(state, request.category_id).await?;
    let options = rd_scheduler::PackageOptions {
        category_id: request.category_id,
        priority: request.priority.unwrap_or_default(),
        paused: request.paused,
        address_reach,
    };
    let file = if let Some(destination) = destination {
        state
            .scheduler
            .enqueue_direct_to_with_network(
                url,
                package_name,
                file_name,
                destination,
                account_id,
                request.proxy_profile_id,
                options,
            )
            .await?
    } else {
        state
            .scheduler
            .enqueue_direct_with_network(
                url,
                package_name,
                file_name,
                account_id,
                request.proxy_profile_id,
                options,
            )
            .await?
    };
    Ok(file)
}

pub(super) async fn validate_network_selection(
    state: &AppState,
    account_id: Option<rd_core::AccountId>,
    proxy_id: Option<rd_core::ProxyProfileId>,
) -> Result<(), ApiError> {
    if let Some(account_id) = account_id {
        let account = state
            .database
            .list_accounts()
            .await?
            .into_iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| {
                ApiError::bad_request("account.not_found", "Provider account not found")
            })?;
        if !account.enabled {
            return Err(ApiError::bad_request(
                "account.disabled",
                "Provider account is disabled",
            ));
        }
    }
    if let Some(proxy_id) = proxy_id
        && !state
            .database
            .list_proxy_profiles()
            .await?
            .iter()
            .any(|profile| profile.id == proxy_id)
    {
        return Err(ApiError::bad_request(
            "proxy.not_found",
            "Proxy profile not found",
        ));
    }
    Ok(())
}

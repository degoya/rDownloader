//! A direct download request: validated and queued as a package of its own.

use super::*;

#[utoipa::path(post, path = "/api/v1/downloads", tag = "downloads", request_body = CreateDownloadRequest, responses((status = 201, body = rd_core::DownloadFile)))]
pub async fn create_download(
    State(state): State<AppState>,
    Json(request): Json<CreateDownloadRequest>,
) -> Result<(StatusCode, Json<rd_core::DownloadFile>), ApiError> {
    let file = create_download_inner(&state, request).await?;
    Ok((StatusCode::CREATED, Json(file)))
}

pub async fn create_download_inner(
    state: &AppState,
    request: CreateDownloadRequest,
) -> Result<rd_core::DownloadFile, ApiError> {
    let url = Url::parse(&request.url)
        .map(rd_collector::canonical_url)
        .map_err(|error| ApiError::bad_request("download.url_invalid", error.to_string()))?;
    if url.scheme() == "magnet" {
        let name = request
            .package_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| crate::torrent_intake::magnet_name(&url));
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
    let inferred = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|value| !value.is_empty())
        .unwrap_or("download.bin")
        .to_owned();
    // A URL-derived package name doubles as the folder name; extensions are stripped.
    let package_name = request
        .package_name
        .unwrap_or_else(|| rd_files::package_name_from_file_name(&inferred));
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

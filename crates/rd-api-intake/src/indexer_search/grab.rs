//! Taking chosen hits into the LinkGrabber: an NZB as an NZB import, a torrent (RD-1100-03) as
//! the package an uploaded `.torrent` or a pasted magnet becomes.

use super::*;

/// What one hit became. Lives for one request; boxing the import would buy nothing.
#[allow(clippy::large_enum_variant)]
pub(super) enum Grabbed {
    Nzb(rd_core::NzbImport),
    Torrent(Vec<rd_core::CollectorPackage>),
}

/// Fetches the chosen hits and puts each into the LinkGrabber (RD-180-19): an NZB as an NZB
/// import, a torrent as a LinkGrabber package (RD-1100-03).
///
/// Each hit on its own: one that fails is reported with a stable code and the others still
/// arrive, because a person who picked ten releases wants the nine that worked.
#[utoipa::path(
    post,
    path = "/api/v1/indexers/grab",
    tag = "indexers",
    request_body = IndexerGrabRequest,
    responses(
        (status = 200, body = IndexerGrabResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn grab_indexer_results(
    State(state): State<AppState>,
    Json(request): Json<IndexerGrabRequest>,
) -> Result<Json<IndexerGrabResponse>, ApiError> {
    if request.items.is_empty() {
        return Err(ApiError::bad_request(
            "indexer.grab_empty",
            "Choose at least one result",
        ));
    }
    if request.items.len() > MAX_GRAB_ITEMS {
        return Err(ApiError::unprocessable(
            "indexer.grab_too_many",
            "Too many results in one request",
        )
        .with_param("maximum", MAX_GRAB_ITEMS));
    }
    if let Some(category_id) = request.category_id
        && !state
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == category_id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    let mut response = IndexerGrabResponse {
        imports: Vec::new(),
        torrents: Vec::new(),
        failed: Vec::new(),
    };
    // One after the other rather than at once: the requests go to the same few servers, which
    // count them, and a burst is how a download limit is met in the middle of a selection.
    for item in request.items {
        match grab_one(&state, &item, request.category_id).await {
            Ok(Grabbed::Nzb(import)) => response.imports.push(import),
            Ok(Grabbed::Torrent(packages)) => response.torrents.extend(packages),
            Err(error) => {
                tracing::info!(code = error.code(), "indexer grab failed");
                response.failed.push(IndexerGrabFailure {
                    title: item.title,
                    error: error.into_message(),
                });
            }
        }
    }
    Ok(Json(response))
}

pub(super) async fn grab_one(
    state: &AppState,
    item: &IndexerGrabItem,
    category_id: Option<CategoryId>,
) -> Result<Grabbed, ApiError> {
    let indexer = crate::indexer_handlers::stored(state, item.indexer_id).await?;
    let raw = item.download.trim();
    let named = url::Url::parse(raw).map_err(|_| invalid_download())?;
    // A Torznab hit without a file of its own is its magnet.
    if named.scheme() == "magnet" {
        return take_magnet(state, &indexer, magnet_url(raw)?, item, category_id).await;
    }
    let fetched = match fetch_download(state, &indexer, raw, named).await {
        Ok(Downloaded::Document(fetched)) => fetched,
        // Prowlarr answers a magnet-only hit's download with `301 Location: magnet:…` and its
        // feed names no magnet beside it (RD-1240-33): the redirect's target is the torrent.
        Ok(Downloaded::Magnet(magnet)) => {
            return take_magnet(state, &indexer, magnet, item, category_id).await;
        }
        // Prowlarr answers some downloads with a redirect to a magnet, which no HTTP client
        // follows; the magnet the hit carried is the same torrent.
        Err(error) if item.magnet.is_some() => {
            tracing::info!(
                code = error.code(),
                "indexer download failed; the hit's magnet is taken instead"
            );
            let magnet = magnet_url(item.magnet.as_deref().unwrap_or_default())?;
            return take_magnet(state, &indexer, magnet, item, category_id).await;
        }
        Err(error) => return Err(error),
    };
    // A refusal arrives inside a `200 OK`, as headers or as an error document; without the
    // check the person would be told the NZB is broken.
    if let Some(refusal) = crate::collector_enqueue::indexer_refusal(&fetched) {
        return Err(ApiError::bad_gateway("indexer.refused", refusal)
            .with_param("indexer", indexer.name.clone()));
    }
    // Decided by the bytes, like a file handed over by the browser: the hit's own word on what
    // it is only chose how the row was drawn.
    if crate::capture_file::looks_like_torrent(&fetched.bytes) {
        return take_torrent(state, &indexer, &fetched.bytes, item, category_id).await;
    }
    if let Some(refusal) = std::str::from_utf8(&fetched.bytes)
        .ok()
        .and_then(rd_subscription::indexer_refusal)
    {
        return Err(refusal_error(&refusal, &indexer));
    }
    let title = item.title.trim();
    let file_name = if title.is_empty() {
        "indexer.nzb".to_owned()
    } else {
        format!("{title}.nzb")
    };
    crate::nzb_handlers::store_nzb_import(
        state,
        &fetched.bytes,
        &file_name,
        category_id,
        // The person searched and chose this release, as they would upload a file.
        rd_core::IngressSource::Manual,
        None,
    )
    .await
    .map(Grabbed::Nzb)
}

fn invalid_download() -> ApiError {
    ApiError::bad_request(
        "indexer.download_invalid",
        "The result's address is not an http or https URL",
    )
}

/// What a hit's download answered: the file, or the magnet it redirected to.
enum Downloaded {
    Document(rd_http::FetchedDocument),
    Magnet(url::Url),
}

/// Fetches a hit's download: with the key on the indexer's own server, under the address guard
/// anywhere else.
async fn fetch_download(
    state: &AppState,
    indexer: &Indexer,
    raw: &str,
    named: url::Url,
) -> Result<Downloaded, ApiError> {
    if !matches!(named.scheme(), "http" | "https") || named.host_str().is_none() {
        return Err(invalid_download());
    }
    let own_server = named.origin() == indexer.url.origin();
    let url = if raw.contains(KEY_PLACEHOLDER) {
        // The key goes to the indexer's own server and nowhere else.
        if !own_server {
            return Err(ApiError::forbidden(
                "indexer.download_foreign",
                "The result's address is not on the indexer's server",
            )
            .with_param("indexer", indexer.name.clone()));
        }
        let key = crate::indexer_handlers::api_key(state, indexer).await?;
        let encoded: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
        url::Url::parse(&raw.replace(KEY_PLACEHOLDER, &encoded)).map_err(|_| invalid_download())?
    } else {
        named
    };
    let network = if own_server {
        // The indexer's own server is the person's word, like its search: their own network
        // included.
        state.scheduler.direct_client(&url).await
    } else {
        // Anything else was proposed by the indexer's answer, and is held to the rule the
        // LinkGrabber's proposed links keep to: never this machine, not the local network.
        let policy = state.scheduler.remote_address_policy(false);
        if let Err(rd_http::TargetRefusal::Refused(_)) =
            rd_http::check_target(&policy, &rd_http::SystemLookup, &url).await
        {
            return Err(ApiError::forbidden(
                "indexer.download_address_refused",
                "The result's address points at this machine or into your own network",
            ));
        }
        state.scheduler.guarded_client(&url, policy).await
    }
    .map_err(|error| {
        ApiError::bad_gateway("indexer.unreachable", error.to_string())
            .with_param("reason", "client")
    })?;
    match rd_http::fetch_document(
        &network.client,
        url.clone(),
        &network.headers,
        rd_collector::MAX_NZB_BYTES,
    )
    .await
    {
        Ok(document) => Ok(Downloaded::Document(document)),
        Err(error) => match error.downcast_ref::<rd_http::UnfollowedRedirect>() {
            // Only a magnet: no client follows one, and any other scheme is nothing to take.
            Some(redirect) if redirect.location.starts_with("magnet:") => {
                redirected_magnet(&redirect.location).map(Downloaded::Magnet)
            }
            _ => Err(ApiError::bad_gateway(
                "indexer.nzb_fetch_failed",
                format!("{error} ({})", rd_core::redact_url(&url)),
            )
            .with_param("indexer", indexer.name.clone())),
        },
    }
}

/// The magnet a download redirected to, held to more than a hit's own: it names a BitTorrent
/// info hash (`xt=urn:btih:`), because nothing else stands behind it.
fn redirected_magnet(location: &str) -> Result<url::Url, ApiError> {
    let magnet = magnet_url(location)?;
    if magnet
        .query_pairs()
        .any(|(key, value)| key == "xt" && value.starts_with("urn:btih:"))
    {
        Ok(magnet)
    } else {
        Err(ApiError::bad_request(
            "indexer.magnet_invalid",
            "Not a magnet link",
        ))
    }
}

/// A magnet a hit named: the scheme and an `xt` topic, nothing else is checked -- the swarm is
/// what tells whether it is real.
fn magnet_url(raw: &str) -> Result<url::Url, ApiError> {
    url::Url::parse(raw.trim())
        .ok()
        .filter(|url| {
            url.scheme() == "magnet"
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "xt" && value.starts_with("urn:"))
        })
        .ok_or_else(|| ApiError::bad_request("indexer.magnet_invalid", "Not a magnet link"))
}

/// The name a grabbed torrent's package carries: the hit's title, which is what the person chose.
fn package_name(item: &IndexerGrabItem) -> Option<String> {
    Some(item.title.trim().to_owned()).filter(|title| !title.is_empty())
}

/// A downloaded `.torrent` as the package an uploaded one becomes, file tree included.
async fn take_torrent(
    state: &AppState,
    indexer: &Indexer,
    bytes: &[u8],
    item: &IndexerGrabItem,
    category_id: Option<CategoryId>,
) -> Result<Grabbed, ApiError> {
    crate::torrent_intake::ensure_torrent_service_enabled(state).await?;
    if bytes.len() > rd_torrent::MAX_TORRENT_BYTES {
        return Err(ApiError::bad_request(
            "torrent.file_too_large",
            "Torrent file exceeds the 16 MiB limit",
        ));
    }
    rd_torrent::parse_torrent(bytes)
        .map_err(|error| ApiError::bad_request("torrent.file_invalid", format!("{error:#}")))?;
    let (_, packages, _) = crate::torrent_intake::add_torrent_to_collector(
        &state.database,
        &state.torrent,
        bytes,
        rd_core::IngressSource::Manual,
        Some(indexer.name.clone()),
        package_name(item),
        category_id,
        None,
    )
    .await?;
    Ok(Grabbed::Torrent(packages))
}

/// A magnet as the LinkGrabber link a pasted one becomes; its online check fetches the file
/// tree from the swarm.
async fn take_magnet(
    state: &AppState,
    indexer: &Indexer,
    magnet: url::Url,
    item: &IndexerGrabItem,
    category_id: Option<CategoryId>,
) -> Result<Grabbed, ApiError> {
    crate::torrent_intake::ensure_torrent_service_enabled(state).await?;
    let name = package_name(item).unwrap_or_else(|| crate::torrent_intake::magnet_name(&magnet));
    let (batch, packages, _) = state
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: Some(indexer.name.clone()),
            package_name: Some(name.clone()),
            password: None,
            passwords: Vec::new(),
            category_id,
            priority: None,
            urls: vec![magnet],
            providers: vec![Some(rd_core::TORRENT_PROVIDER.to_owned())],
            file_names: vec![Some(rd_files::sanitize_file_name(&name))],
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await?;
    state.link_check.check_batch(batch.id).await;
    Ok(Grabbed::Torrent(packages))
}

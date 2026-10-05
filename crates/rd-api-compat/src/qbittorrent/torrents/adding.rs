//! `torrents/add`: a torrent file or a link handed over by an *arr client.

use super::*;

/// `POST /api/v2/torrents/add`: a `.torrent` upload, a magnet, or both.
///
/// qBittorrent queues immediately, so this bypasses the LinkGrabber review the native
/// upload keeps — an automation client has already decided what it wants and polls for the
/// torrent to appear. It answers `Ok.`, which is the only body clients check.
pub(crate) async fn add(State(state): State<AppState>, multipart: Option<Multipart>) -> Response {
    let Some(mut multipart) = multipart else {
        return failure();
    };
    let mut files: Vec<axum::body::Bytes> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    let mut category: Option<String> = None;
    let mut paused = false;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or_default().to_owned();
                match name.as_str() {
                    "torrents" | "fileselect[]" => match field.bytes().await {
                        Ok(bytes) => files.push(bytes),
                        Err(_) => return failure(),
                    },
                    "urls" => {
                        if let Ok(text) = field.text().await {
                            urls.extend(
                                text.lines()
                                    .map(str::trim)
                                    .filter(|line| !line.is_empty())
                                    .map(str::to_owned),
                            );
                        }
                    }
                    "category" => category = field.text().await.ok(),
                    "paused" | "stopped" => {
                        paused = field
                            .text()
                            .await
                            .is_ok_and(|value| value.trim().eq_ignore_ascii_case("true"));
                    }
                    _ => {}
                }
            }
            Ok(None) => break,
            Err(_) => return failure(),
        }
    }
    if files.is_empty() && urls.is_empty() {
        return failure();
    }
    let category_id = resolve_category(&state, category.as_deref()).await;
    for content in &files {
        if add_file(&state, content, category_id, paused)
            .await
            .is_err()
        {
            return failure();
        }
    }
    for url in &urls {
        if add_url(&state, url, category_id).await.is_err() {
            return failure();
        }
    }
    ok()
}

pub(super) async fn add_file(
    state: &AppState,
    content: &[u8],
    category_id: Option<rd_core::CategoryId>,
    paused: bool,
) -> anyhow::Result<()> {
    let (_, packages, _) = crate::torrent_intake::add_torrent_to_collector(
        &state.database,
        &state.torrent,
        content,
        rd_core::IngressSource::Api,
        Some("qbittorrent".to_owned()),
        None,
        category_id,
        None,
    )
    .await?;
    for package in packages {
        crate::collector_enqueue::enqueue_package(state, package.id, paused, None)
            .await
            .map_err(|error| anyhow::anyhow!("{}", error.message()))?;
    }
    Ok(())
}

pub(super) async fn add_url(
    state: &AppState,
    url: &str,
    category_id: Option<rd_core::CategoryId>,
) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url)?;
    // Only magnets: an `http(s)` entry here would ask the service to fetch a URL chosen by
    // the caller, which is the same refusal the SABnzbd adapter makes for `addurl`.
    anyhow::ensure!(
        parsed.scheme() == "magnet",
        "only magnet links are accepted"
    );
    let name = crate::torrent_intake::magnet_name(&parsed);
    crate::torrent_intake::enqueue_torrent_with(
        &state.database,
        &state.scheduler,
        parsed,
        name,
        None,
        category_id,
        rd_core::DownloadPriority::Normal,
    )
    .await
    .map_err(|error| anyhow::anyhow!("{}", error.message()))?;
    Ok(())
}

pub(super) async fn resolve_category(
    state: &AppState,
    name: Option<&str>,
) -> Option<rd_core::CategoryId> {
    let name = name.map(str::trim).filter(|name| !name.is_empty())?;
    let categories = match state.database.list_categories().await {
        Ok(categories) => categories,
        Err(error) => {
            // Queued without a category rather than refused; said, so a torrent that lands in
            // the wrong folder is explained (audit 1.9.1, API-13).
            tracing::warn!(
                error = %format!("{error:#}"),
                category = name,
                "the qBittorrent adapter could not read the categories"
            );
            return None;
        }
    };
    categories
        .into_iter()
        .find(|category| category.name.eq_ignore_ascii_case(name))
        .map(|category| category.id)
}

/// qBittorrent's failure body for `torrents/add`.
pub(super) fn failure() -> Response {
    (
        axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "Torrent file is not valid",
    )
        .into_response()
}

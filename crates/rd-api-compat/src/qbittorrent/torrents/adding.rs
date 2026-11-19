//! `torrents/add`: a torrent file or a link handed over by an *arr client.

use axum::extract::FromRequest;

use super::*;

/// What one `torrents/add` call asks for, whichever of the two encodings it came in.
#[derive(Default)]
struct AddRequest {
    files: Vec<axum::body::Bytes>,
    urls: Vec<String>,
    category: Option<String>,
    paused: bool,
}

impl AddRequest {
    /// One text field. Both encodings go through here, so they differ only in how a field is
    /// read, never in what it means or how it is checked.
    fn text_field(&mut self, name: &str, value: &str) {
        match name {
            "urls" => self.urls.extend(
                value
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            ),
            "category" => self.category = Some(value.to_owned()),
            "paused" | "stopped" => self.paused = value.trim().eq_ignore_ascii_case("true"),
            // `savepath`, `tags`, `sequentialDownload` and the rest are read by qBittorrent
            // and ignored here: categories and routing rules decide where a download lands.
            _ => {}
        }
    }
}

/// `POST /api/v2/torrents/add`: a `.torrent` upload, a magnet, or both.
///
/// qBittorrent queues immediately, so this bypasses the LinkGrabber review the native
/// upload keeps — an automation client has already decided what it wants and polls for the
/// torrent to appear. It answers an empty 200, as older qBittorrent does; clients fail only on
/// `Fails.`.
///
/// The body is `multipart/form-data` or `application/x-www-form-urlencoded`, as qBittorrent
/// takes it: Sonarr and Radarr send a magnet as a plain form (`urls`, `category`, `paused`) and
/// only a `.torrent` upload as multipart (RD-1430-12). A file travels in multipart only; any
/// other content type is refused like an unreadable torrent.
pub(crate) async fn add(
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Response {
    let Some(AddRequest {
        files,
        urls,
        category,
        paused,
    }) = read_request(&state, request).await
    else {
        return failure();
    };
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

/// Reads the body by its media type; `None` for any other type or a body that cannot be read.
async fn read_request(state: &AppState, request: axum::extract::Request) -> Option<AddRequest> {
    let media_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())?;
    match media_type.as_str() {
        "multipart/form-data" => {
            let multipart = Multipart::from_request(request, state).await.ok()?;
            read_multipart(multipart).await
        }
        "application/x-www-form-urlencoded" => {
            let body = axum::body::Bytes::from_request(request, state).await.ok()?;
            let mut add = AddRequest::default();
            for (name, value) in url::form_urlencoded::parse(&body) {
                add.text_field(&name, &value);
            }
            Some(add)
        }
        _ => None,
    }
}

async fn read_multipart(mut multipart: Multipart) -> Option<AddRequest> {
    let mut add = AddRequest::default();
    while let Some(field) = multipart.next_field().await.ok()? {
        let name = field.name().unwrap_or_default().to_owned();
        if matches!(name.as_str(), "torrents" | "fileselect[]") {
            add.files.push(field.bytes().await.ok()?);
        } else if let Ok(text) = field.text().await {
            add.text_field(&name, &text);
        }
    }
    Some(add)
}

pub(super) async fn add_file(
    state: &AppState,
    content: &[u8],
    category_id: Option<rd_core::CategoryId>,
    paused: bool,
) -> anyhow::Result<()> {
    let (_, packages, _) = crate::torrent_intake::add_torrent_to_collector(
        &state.database,
        &state.queue.torrent,
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

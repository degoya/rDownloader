//! The `torrents/*` endpoints: listing, inspecting, adding and acting on torrents.

use axum::{
    extract::{Multipart, Query, State},
    response::{IntoResponse, Response},
};

use super::{TorrentQuery, map, ok};
use crate::{AppState, dto::DownloadBulkAction};

/// Collects every torrent package with its job row and real info hash.
async fn views(state: &AppState) -> Vec<map::TorrentView> {
    let Ok(packages) = state.database.list_packages().await else {
        return Vec::new();
    };
    let Ok(downloads) = state.database.list_downloads().await else {
        return Vec::new();
    };
    let Ok(torrent_states) = state.database.all_download_torrent_states().await else {
        return Vec::new();
    };
    let categories = state.database.list_categories().await.unwrap_or_default();
    downloads
        .into_iter()
        .filter_map(|download| {
            let hash = torrent_states
                .iter()
                .find(|(id, _)| *id == download.id)
                .and_then(|(_, job)| job.metadata.as_ref())
                .map(|metadata| map::normalize_hash(&metadata.info_hash))
                .or_else(|| map::hash_from_magnet(&download.source))?;
            let package = packages
                .iter()
                .find(|package| package.id == download.package_id)?
                .clone();
            let category = categories
                .iter()
                .find(|category| Some(category.id) == package.category_id)
                .map(|category| category.name.clone())
                .unwrap_or_default();
            Some(map::TorrentView {
                package,
                download,
                hash,
                category,
            })
        })
        .collect()
}

/// Form fields of a `POST`, which is where qBittorrent clients put their parameters.
///
/// The query string is the documented spelling and the form body is what clients actually
/// send, so every action reads both. Only looking at the query made `delete` a no-op for a
/// real client while every hand-written request still worked.
pub(crate) fn form(body: &str) -> std::collections::HashMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

/// The hashes named by a request, normalised. `all` means every torrent.
fn requested(
    query: &TorrentQuery,
    form: &std::collections::HashMap<String, String>,
    views: &[map::TorrentView],
) -> Vec<String> {
    let raw = query
        .hashes
        .as_deref()
        .or(query.hash.as_deref())
        .or_else(|| form.get("hashes").map(String::as_str))
        .or_else(|| form.get("hash").map(String::as_str))
        .unwrap_or_default();
    if raw.trim().eq_ignore_ascii_case("all") {
        return views.iter().map(|view| view.hash.clone()).collect();
    }
    raw.split('|')
        .flat_map(|part| part.split(','))
        .map(map::normalize_hash)
        .filter(|hash| !hash.is_empty())
        .collect()
}

pub(crate) async fn info(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
) -> Response {
    let views = views(&state).await;
    let slots: Vec<serde_json::Value> = views
        .iter()
        .filter(|view| matches_filter(view, &query))
        .map(entry)
        .collect();
    axum::Json(slots).into_response()
}

/// Applies the `category` and `filter` narrowing a client asks for.
fn matches_filter(view: &map::TorrentView, query: &TorrentQuery) -> bool {
    if let Some(filter) = query.filter.as_deref() {
        let finished = map::is_finished(&view.package);
        let matches = match filter {
            "completed" => finished,
            "downloading" => !finished,
            // Everything else — `all`, `paused`, `resumed`, `stalled` and the rest — is
            // answered with the full list rather than a guess at our equivalent.
            _ => true,
        };
        if !matches {
            return false;
        }
    }
    // qBittorrent's reading: no `category` parameter means any category, and an empty one
    // means the torrents that have none. Case-insensitive because `add` resolves the name
    // that way, so a client asking for `TV` finds what it queued under `tv`.
    query
        .category
        .as_deref()
        .is_none_or(|category| category.eq_ignore_ascii_case(category_of(view)))
}

/// The category name of a torrent, or the empty string qBittorrent uses for "none".
fn category_of(view: &map::TorrentView) -> &str {
    &view.category
}

fn entry(view: &map::TorrentView) -> serde_json::Value {
    let total = view.download.total_bytes.map_or(0, rd_core::ByteCount::get);
    let done = view.download.committed_bytes.get();
    serde_json::json!({
        "hash": view.hash,
        "name": view.package.name,
        "size": total,
        "total_size": total,
        "completed": done,
        "downloaded": done,
        "uploaded": 0,
        "progress": map::progress(&view.download),
        "eta": 8_640_000,
        "state": map::state(&view.package, &view.download),
        "category": category_of(view),
        "tags": "",
        "save_path": view.package.destination,
        "content_path": view.package.destination,
        "download_path": view.package.destination,
        "dlspeed": 0,
        "upspeed": 0,
        "priority": 0,
        "num_seeds": 0,
        "num_leechs": 0,
        "ratio": 0.0,
        "seq_dl": false,
        "f_l_piece_prio": false,
        "force_start": false,
        "super_seeding": false,
        "auto_tmm": false,
        "time_active": 0,
        "seeding_time": 0,
        "added_on": view.package.created_at.timestamp(),
        "completion_on": if map::is_finished(&view.package) {
            view.package.created_at.timestamp()
        } else {
            0
        },
        "amount_left": total.saturating_sub(done),
    })
}

pub(crate) async fn properties(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
) -> Response {
    let views = views(&state).await;
    let hashes = requested(&query, &form(""), &views);
    let Some(view) = views.iter().find(|view| hashes.contains(&view.hash)) else {
        return not_found();
    };
    let total = view.download.total_bytes.map_or(0, rd_core::ByteCount::get);
    axum::Json(serde_json::json!({
        "save_path": view.package.destination,
        "creation_date": view.package.created_at.timestamp(),
        "piece_size": 0,
        "comment": "",
        "total_wasted": 0,
        "total_uploaded": 0,
        "total_downloaded": view.download.committed_bytes.get(),
        "total_size": total,
        "up_limit": -1,
        "dl_limit": -1,
        "time_elapsed": 0,
        "seeding_time": 0,
        "nb_connections": 0,
        "share_ratio": 0.0,
        "addition_date": view.package.created_at.timestamp(),
        "is_private": false,
    }))
    .into_response()
}

pub(crate) async fn files(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
) -> Response {
    let views = views(&state).await;
    let hashes = requested(&query, &form(""), &views);
    let Some(view) = views.iter().find(|view| hashes.contains(&view.hash)) else {
        return not_found();
    };
    let Ok(Some(job)) = state
        .database
        .download_torrent_state(view.download.id)
        .await
    else {
        return axum::Json(Vec::<serde_json::Value>::new()).into_response();
    };
    // Resolved through the same evaluator the native API and the UI use, so the selection
    // a client sees is the one the review step actually produced — including files an
    // exclusion pattern removed rather than only the explicitly unchecked ones.
    let files: Vec<serde_json::Value> = job
        .metadata
        .as_ref()
        .map(|metadata| {
            rd_core::resolve_plan(metadata, &job.plan)
                .files
                .iter()
                .map(|file| {
                    serde_json::json!({
                        "index": file.index,
                        "name": file.path.join("/"),
                        "size": file.length.get(),
                        "progress": map::progress(&view.download),
                        // A deselected file reports qBittorrent's "do not download"
                        // priority, which is how a client learns it will not arrive.
                        "priority": u8::from(file.included),
                        "is_seed": false,
                        "piece_range": [0, 0],
                        "availability": 0.0,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    axum::Json(files).into_response()
}

pub(crate) async fn categories(State(state): State<AppState>) -> Response {
    let mut map = serde_json::Map::new();
    if let Ok(categories) = state.database.list_categories().await {
        for category in categories {
            map.insert(
                category.name.clone(),
                serde_json::json!({ "name": category.name, "savePath": "" }),
            );
        }
    }
    axum::Json(serde_json::Value::Object(map)).into_response()
}

/// Categories are configured in the application, not by a download client.
///
/// Answering `Ok.` rather than failing: a client creates its category on connect and treats
/// a refusal as a broken server, while the routing rules decide where files land anyway.
pub(crate) async fn create_category() -> Response {
    ok()
}

pub(crate) async fn set_category() -> Response {
    ok()
}

pub(crate) async fn pause(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
    body: String,
) -> Response {
    act(&state, &query, &body, DownloadBulkAction::Pause).await
}

pub(crate) async fn resume(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
    body: String,
) -> Response {
    act(&state, &query, &body, DownloadBulkAction::Resume).await
}

async fn act(
    state: &AppState,
    query: &TorrentQuery,
    body: &str,
    action: DownloadBulkAction,
) -> Response {
    let views = views(state).await;
    let hashes = requested(query, &form(body), &views);
    let targets: Vec<rd_core::DownloadId> = views
        .iter()
        .filter(|view| hashes.contains(&view.hash))
        .map(|view| view.download.id)
        .collect();
    if targets.is_empty() {
        return ok();
    }
    // `apply_download_action` reports a per-file refusal inside its answer rather than as an
    // error, so a run in which nothing at all succeeded still comes back `Ok`. Both readings
    // have to reach the client: Sonarr and Radarr mark an item handled on any 2xx and never
    // return to it, so an `Ok.` after a pause or resume that did nothing loses the item.
    match crate::download_handlers::apply_download_action(state, action, targets.clone()).await {
        Ok(result) if result.affected == 0 => {
            tracing::warn!(
                ?action,
                ids = ?targets,
                errors = ?result.errors,
                "qBittorrent action applied to nothing"
            );
            failed("The action could not be applied")
        }
        Ok(result) => {
            // Partly applied: the protocol has no shape for "three of five", and the ones
            // that worked must not be undone by a retry, so the client is told it succeeded
            // and the rest is left in the log.
            if !result.errors.is_empty() {
                tracing::warn!(
                    ?action,
                    errors = ?result.errors,
                    "qBittorrent action failed for some torrents"
                );
            }
            ok()
        }
        Err(error) => {
            tracing::warn!(
                ?action,
                ids = ?targets,
                error = error.message(),
                "qBittorrent action refused"
            );
            failed(error.message())
        }
    }
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
    body: String,
) -> Response {
    let views = views(&state).await;
    let hashes = requested(&query, &form(&body), &views);
    let ids: Vec<rd_core::PackageId> = views
        .iter()
        .filter(|view| hashes.contains(&view.hash))
        .map(|view| view.package.id)
        .collect();
    if ids.is_empty() {
        return ok();
    }
    // As in the SABnzbd adapter: the native removal path cancels and cleans up part files.
    // `deleteFiles=true` is not honoured for finished data — deleting files a client has
    // just imported stays a deliberate action inside the application.
    // Forced: these clients name one item and expect it gone, and their protocols have no
    // way to carry back "it is still running, confirm first".
    // The failure is not forced, though: a removal that did not happen used to be answered
    // with `Ok.`, and the client then struck the item off and never asked again.
    match crate::package_handlers::remove_packages(&state, ids.clone(), true).await {
        Ok(_) => ok(),
        Err(error) => {
            tracing::warn!(?ids, error = error.message(), "qBittorrent removal failed");
            failed(error.message())
        }
    }
}

/// What the adapter answers when the action itself failed.
///
/// qBittorrent's own API carries nothing but the status code here, and that is what the
/// clients read: any non-2xx means "not done" and they try again, which is exactly the
/// behaviour a failed pause, resume or removal needs. The reason goes in the body, where the
/// module's other failures put it, and in the log.
fn failed(reason: &str) -> Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        reason.to_owned(),
    )
        .into_response()
}

fn not_found() -> Response {
    (axum::http::StatusCode::NOT_FOUND, "Torrent not found").into_response()
}

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

async fn add_file(
    state: &AppState,
    content: &[u8],
    category_id: Option<rd_core::CategoryId>,
    paused: bool,
) -> anyhow::Result<()> {
    let (_, packages, _) = crate::torrent_handlers::add_torrent_to_collector(
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

async fn add_url(
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
    let name = crate::torrent_handlers::magnet_name(&parsed);
    crate::torrent_handlers::enqueue_torrent_with(
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

async fn resolve_category(state: &AppState, name: Option<&str>) -> Option<rd_core::CategoryId> {
    let name = name.map(str::trim).filter(|name| !name.is_empty())?;
    state
        .database
        .list_categories()
        .await
        .ok()?
        .into_iter()
        .find(|category| category.name.eq_ignore_ascii_case(name))
        .map(|category| category.id)
}

/// qBittorrent's failure body for `torrents/add`.
fn failure() -> Response {
    (
        axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "Torrent file is not valid",
    )
        .into_response()
}

/// `POST /api/v2/torrents/filePrio`: include or exclude files of a queued torrent.
///
/// qBittorrent expresses "do not download" as priority `0` and everything else as a tier.
/// Only the inclusion half is honoured, because that is the decision our review model
/// records; the request goes through the same validation and engine call as the native
/// plan endpoint, so a selection that would leave nothing to download is refused here too.
pub(crate) async fn file_priority(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
    body: String,
) -> Response {
    let form = form(&body);
    let hash = query
        .hash
        .clone()
        .or_else(|| form.get("hash").cloned())
        .map(|value| map::normalize_hash(&value))
        .unwrap_or_default();
    let views = views(&state).await;
    let Some(view) = views.iter().find(|view| view.hash == hash) else {
        return not_found();
    };
    let Some(indices) = form.get("id") else {
        return bad_request();
    };
    let indices: Vec<u32> = indices
        .split('|')
        .filter_map(|value| value.trim().parse().ok())
        .collect();
    let exclude = form
        .get("priority")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .is_some_and(|priority| priority == 0);
    let (included, excluded) = if exclude {
        (Vec::new(), indices)
    } else {
        (indices, Vec::new())
    };
    let request = crate::torrent_control::TorrentPlanRequest::selection(included, excluded);
    match crate::torrent_control::apply_download_plan(&state, view.download.id, request).await {
        Ok(_) => ok(),
        Err(_) => bad_request(),
    }
}

fn bad_request() -> Response {
    (axum::http::StatusCode::BAD_REQUEST, "Priority is not valid").into_response()
}

/// `POST /api/v2/torrents/setShareLimits`: ratio and seeding-time targets.
///
/// Mapped onto the per-torrent seeding override rather than acknowledged and forgotten: a
/// client that sets a seed goal expects the torrent to actually stop. qBittorrent's
/// sentinels are kept — `-1` means "use the global setting", `-2` means "no limit" — and a
/// value outside our accepted range is refused rather than clamped, because silently
/// seeding to a different target than the one asked for is worse than a visible failure.
pub(crate) async fn set_share_limits(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
    body: String,
) -> Response {
    let form = form(&body);
    let views = views(&state).await;
    let hashes = requested(&query, &form, &views);
    let ratio = form
        .get("ratioLimit")
        .and_then(|value| value.trim().parse::<f64>().ok());
    let minutes = form
        .get("seedingTimeLimit")
        .and_then(|value| value.trim().parse::<i64>().ok());
    let request = crate::torrent_control::SeedingPolicyRequest {
        enabled: Some(true),
        // -1 asks for the global default, which is what "no override" already means here;
        // -2 is "no ratio limit", whose stored form is a ratio of zero, the value that
        // switches the ratio stop off. Anything else goes through as it is and is refused
        // below if it lies outside the accepted range.
        ratio: ratio.and_then(|value| {
            if value == -1.0 {
                None
            } else if value == -2.0 {
                Some(0.0)
            } else {
                Some(value)
            }
        }),
        time_minutes: minutes
            .filter(|value| *value > 0)
            .and_then(|value| u32::try_from(value).ok()),
        time_unlimited: (minutes == Some(-2)).then_some(true),
    };
    for view in views.iter().filter(|view| hashes.contains(&view.hash)) {
        let request = crate::torrent_control::SeedingPolicyRequest {
            enabled: request.enabled,
            ratio: request.ratio,
            time_minutes: request.time_minutes,
            time_unlimited: request.time_unlimited,
        };
        if crate::torrent_control::apply_download_seeding(&state, view.download.id, request)
            .await
            .is_err()
        {
            return bad_request();
        }
    }
    ok()
}

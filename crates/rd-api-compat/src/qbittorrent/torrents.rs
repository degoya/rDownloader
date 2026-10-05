//! The `torrents/*` endpoints: listing, inspecting, adding and acting on torrents.

use axum::{
    extract::{Multipart, Query, State},
    response::{IntoResponse, Response},
};

use super::{TorrentQuery, map, ok};
use crate::{AppState, dto::DownloadBulkAction};

mod adding;
mod priorities;

pub(crate) use adding::*;
pub(crate) use priorities::*;

/// Collects every torrent package with its job row and real info hash.
///
/// Three reads and one pass (audit 1.9.1, API-03): the job states and the packages are keyed
/// once, so a list of `n` downloads costs `n` lookups rather than `n` scans of both tables.
///
/// A store that cannot be read answers `503` rather than an empty list. An empty list is a
/// statement -- "you have no torrents" -- and Sonarr and Radarr act on it, marking every one
/// they are waiting for as gone.
async fn views(state: &AppState) -> Result<Vec<map::TorrentView>, Box<Response>> {
    let read = async {
        anyhow::Ok((
            state.database.list_packages().await?,
            state.database.list_downloads().await?,
            state.database.all_download_torrent_states().await?,
            state.database.list_categories().await?,
        ))
    };
    let (packages, downloads, torrent_states, categories) =
        read.await.map_err(|error| Box::new(unavailable(&error)))?;
    let hashes: std::collections::HashMap<rd_core::DownloadId, String> = torrent_states
        .into_iter()
        .filter_map(|(id, job)| {
            job.metadata
                .as_ref()
                .map(|metadata| (id, map::normalize_hash(&metadata.info_hash)))
        })
        .collect();
    let packages: std::collections::HashMap<rd_core::PackageId, rd_core::DownloadPackage> =
        packages
            .into_iter()
            .map(|package| (package.id, package))
            .collect();
    Ok(downloads
        .into_iter()
        .filter_map(|download| {
            let hash = hashes
                .get(&download.id)
                .cloned()
                .or_else(|| map::hash_from_magnet(&download.source))?;
            let package = packages.get(&download.package_id)?.clone();
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
        .collect())
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
    let views = match views(&state).await {
        Ok(views) => views,
        Err(unavailable) => return *unavailable,
    };
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
    let views = match views(&state).await {
        Ok(views) => views,
        Err(unavailable) => return *unavailable,
    };
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
    let views = match views(&state).await {
        Ok(views) => views,
        Err(unavailable) => return *unavailable,
    };
    let hashes = requested(&query, &form(""), &views);
    let Some(view) = views.iter().find(|view| hashes.contains(&view.hash)) else {
        return not_found();
    };
    let job = match state
        .database
        .download_torrent_state(view.download.id)
        .await
    {
        Ok(Some(job)) => job,
        Ok(None) => return axum::Json(Vec::<serde_json::Value>::new()).into_response(),
        // Not an empty file list: a client would read it as a torrent without files (API-13).
        Err(error) => return unavailable(&error),
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
    let categories = match state.database.list_categories().await {
        Ok(categories) => categories,
        // An empty map tells the client its categories are gone, and it creates them again.
        Err(error) => return unavailable(&error),
    };
    let mut map = serde_json::Map::new();
    for category in categories {
        map.insert(
            category.name.clone(),
            serde_json::json!({ "name": category.name, "savePath": "" }),
        );
    }
    axum::Json(serde_json::Value::Object(map)).into_response()
}

/// `503` for a store that could not be read, with the cause in the log (audit 1.9.1, API-13).
fn unavailable(error: &anyhow::Error) -> Response {
    tracing::warn!(
        error = %format!("{error:#}"),
        "the qBittorrent adapter could not read the store"
    );
    axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
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
    let views = match views(state).await {
        Ok(views) => views,
        Err(unavailable) => return *unavailable,
    };
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
    let views = match views(&state).await {
        Ok(views) => views,
        Err(unavailable) => return *unavailable,
    };
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
        failure_body(reason),
    )
        .into_response()
}

/// The body [`failed`] answers with: the reason, redacted.
///
/// An error message can quote a signed link or a header it was built from, and this body goes
/// to an automation client and into its log, which is not a place a credential belongs
/// (security audit 2026-09-30).
fn failure_body(reason: &str) -> String {
    rd_core::redact_text(reason)
}

fn not_found() -> Response {
    (axum::http::StatusCode::NOT_FOUND, "Torrent not found").into_response()
}

#[cfg(test)]
mod tests {
    /// A refusal quoting a signed link reaches the client without the signature.
    #[test]
    fn a_failure_body_carries_no_credential() {
        let body = super::failure_body(
            "could not reach https://tracker.example/announce?token=s3cr3t-value&x=1",
        );
        assert!(!body.contains("s3cr3t-value"), "{body}");
        assert!(body.contains("tracker.example"), "{body}");
    }
}

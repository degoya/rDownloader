//! The `queue` mode: listing what is still being worked on, and acting on one slot.

use axum::response::Response;
use rd_core::{DownloadFile, DownloadPackage};

use super::{SabQuery, error, json, map, ok};
use crate::{AppState, dto::DownloadBulkAction};

pub(crate) async fn handle(state: &AppState, query: &SabQuery) -> Response {
    match query.name.as_deref() {
        Some("delete") => delete(state, query).await,
        Some("pause") => act(state, query, DownloadBulkAction::Pause).await,
        Some("resume") => act(state, query, DownloadBulkAction::Resume).await,
        Some(other) => error(&format!("not implemented: queue/{other}")),
        None => list(state).await,
    }
}

async fn list(state: &AppState) -> Response {
    let Ok(packages) = state.database.list_packages().await else {
        return error("queue unavailable");
    };
    let Ok(downloads) = state.database.list_downloads().await else {
        return error("queue unavailable");
    };
    let mut total = 0_u64;
    let mut left = 0_u64;
    let slots: Vec<serde_json::Value> = packages
        .iter()
        .filter(|package| !map::is_history(package))
        .enumerate()
        .map(|(index, package)| {
            let (bytes, done) = totals(&downloads, package);
            total += bytes;
            left += bytes.saturating_sub(done);
            slot(index, package, bytes, done)
        })
        .collect();
    json(serde_json::json!({
        "queue": {
            "status": if slots.is_empty() { "Idle" } else { "Downloading" },
            "paused": false,
            "pause_int": "0",
            "speedlimit": "100",
            "speedlimit_abs": "",
            "noofslots": slots.len(),
            "noofslots_total": slots.len(),
            "limit": 0,
            "start": 0,
            "version": super::REPORTED_VERSION,
            "diskspace1": "1000.00",
            "diskspace2": "1000.00",
            "diskspacetotal1": "1000.00",
            "diskspacetotal2": "1000.00",
            "mb": map::megabytes(total),
            "mbleft": map::megabytes(left),
            "size": map::human_size(total),
            "sizeleft": map::human_size(left),
            "speed": "0",
            "kbpersec": "0.00",
            // No estimate is offered rather than a made-up one: the native API knows the
            // remaining time per download, but the queue-wide figure a client would show
            // here cannot be derived from a snapshot without the current rate.
            "timeleft": "0:00:00",
            "eta": "unknown",
            "slots": slots,
        }
    }))
}

fn slot(index: usize, package: &DownloadPackage, bytes: u64, done: u64) -> serde_json::Value {
    let left = bytes.saturating_sub(done);
    let percentage = if bytes == 0 {
        0
    } else {
        ((done as f64 / bytes as f64) * 100.0).round() as u64
    };
    serde_json::json!({
        "index": index,
        "nzo_id": map::nzo_id(package),
        "filename": package.name,
        "nzbname": package.name,
        "cat": "*",
        "priority": "Normal",
        "script": package.script.clone().unwrap_or_else(|| "None".to_owned()),
        "unpackopts": "3",
        "status": map::queue_status(package),
        "mb": map::megabytes(bytes),
        "mbleft": map::megabytes(left),
        "size": map::human_size(bytes),
        "sizeleft": map::human_size(left),
        "percentage": percentage.to_string(),
        "timeleft": map::time_left(0),
        "eta": "unknown",
        "avg_age": "0d",
        "labels": [],
        "password": "",
    })
}

/// Byte totals of one package: how much it is, and how much is committed.
///
/// A row standing by is left out of both sums. A mirror that stood down and a PAR2 recovery
/// volume held back until a repair asks for it (RD-107-04) are bytes nobody is going to
/// fetch, and counting them would report a package as permanently short of its own size.
fn totals(downloads: &[DownloadFile], package: &DownloadPackage) -> (u64, u64) {
    downloads
        .iter()
        .filter(|file| file.package_id == package.id)
        .filter(|file| file.state != rd_core::DownloadState::Skipped)
        .fold((0, 0), |(total, done), file| {
            (
                total + file.total_bytes.map_or(0, rd_core::ByteCount::get),
                done + file.committed_bytes.get(),
            )
        })
}

async fn act(state: &AppState, query: &SabQuery, action: DownloadBulkAction) -> Response {
    let Some(id) = query.value.as_deref().and_then(map::package_id) else {
        return error("nzo_id not found");
    };
    apply_to_package(state, &[id], action).await
}

async fn delete(state: &AppState, query: &SabQuery) -> Response {
    let Some(value) = query.value.as_deref() else {
        return error("nzo_id not found");
    };
    // SABnzbd accepts `value=all` to clear the queue, and clients use it when a user
    // empties the download client from their side.
    let ids: Vec<rd_core::PackageId> = if value.eq_ignore_ascii_case("all") {
        match state.database.list_packages().await {
            Ok(packages) => packages
                .iter()
                .filter(|package| !map::is_history(package))
                .map(|package| package.id)
                .collect(),
            Err(_) => return error("queue unavailable"),
        }
    } else {
        match map::package_id(value) {
            Some(id) => vec![id],
            None => return error("nzo_id not found"),
        }
    };
    if ids.is_empty() {
        return ok();
    }
    // The native removal path, so part files and cancellation behave exactly as they do
    // when the same package is removed from the web interface.
    // Forced: these clients name one item and expect it gone, and their protocols have no
    // way to carry back "it is still running, confirm first".
    match crate::package_handlers::remove_packages(state, ids, true).await {
        Ok(_) => ok(),
        Err(failure) => error(failure.message()),
    }
}

/// Applies a bulk action to every download of the given packages.
async fn apply_to_package(
    state: &AppState,
    ids: &[rd_core::PackageId],
    action: DownloadBulkAction,
) -> Response {
    let Ok(downloads) = state.database.list_downloads().await else {
        return error("queue unavailable");
    };
    let targets: Vec<rd_core::DownloadId> = downloads
        .iter()
        .filter(|file| ids.contains(&file.package_id))
        .map(|file| file.id)
        .collect();
    if targets.is_empty() {
        return ok();
    }
    match crate::download_handlers::apply_download_action(state, action, targets).await {
        Ok(_) => ok(),
        Err(failure) => error(failure.message()),
    }
}

pub(crate) async fn pause_all(state: &AppState) -> Response {
    all(state, DownloadBulkAction::Pause).await
}

pub(crate) async fn resume_all(state: &AppState) -> Response {
    all(state, DownloadBulkAction::Resume).await
}

/// Pauses or resumes every package that has not reached an end state.
async fn all(state: &AppState, action: DownloadBulkAction) -> Response {
    let Ok(packages) = state.database.list_packages().await else {
        return error("queue unavailable");
    };
    let ids: Vec<rd_core::PackageId> = packages
        .iter()
        .filter(|package| !map::is_history(package))
        .map(|package| package.id)
        .collect();
    apply_to_package(state, &ids, action).await
}

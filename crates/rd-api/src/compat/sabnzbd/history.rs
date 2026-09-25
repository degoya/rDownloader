//! The `history` mode: packages that reached an end state, and removing one.

use axum::response::Response;
use rd_core::{DownloadFile, DownloadPackage, PackageState};

use super::{SabQuery, error, json, map, ok};
use crate::AppState;

pub(crate) async fn handle(state: &AppState, query: &SabQuery) -> Response {
    match query.name.as_deref() {
        Some("delete") => delete(state, query).await,
        Some(other) => error(&format!("not implemented: history/{other}")),
        None => list(state).await,
    }
}

async fn list(state: &AppState) -> Response {
    let Ok(packages) = state.database.list_packages().await else {
        return error("history unavailable");
    };
    let Ok(downloads) = state.database.list_downloads().await else {
        return error("history unavailable");
    };
    let slots: Vec<serde_json::Value> = packages
        .iter()
        .filter(|package| map::is_history(package))
        .enumerate()
        .map(|(index, package)| slot(index, package, &downloads))
        .collect();
    let total: u64 = slots.iter().filter_map(|slot| slot["bytes"].as_u64()).sum();
    json(serde_json::json!({
        "history": {
            "noofslots": slots.len(),
            "ppslots": 0,
            "day_size": map::human_size(total),
            "week_size": map::human_size(total),
            "month_size": map::human_size(total),
            "total_size": map::human_size(total),
            "last_history_update": chrono::Utc::now().timestamp(),
            "version": super::REPORTED_VERSION,
            "slots": slots,
        }
    }))
}

fn slot(index: usize, package: &DownloadPackage, downloads: &[DownloadFile]) -> serde_json::Value {
    let bytes: u64 = downloads
        .iter()
        .filter(|file| file.package_id == package.id)
        .map(|file| file.committed_bytes.get())
        .sum();
    let failed = package.state == PackageState::Failed;
    // `storage` is the field an automation client reads to find the finished files; without
    // it an import silently does nothing. It is the package destination, which is where the
    // native pipeline promotes completed files to.
    serde_json::json!({
        "id": index,
        "nzo_id": map::nzo_id(package),
        "name": package.name,
        "nzb_name": format!("{}.nzb", package.name),
        "category": "*",
        "pp": "D",
        "script": package.script.clone().unwrap_or_else(|| "None".to_owned()),
        "status": if failed { "Failed" } else { "Completed" },
        "fail_message": failure_message(package),
        "storage": package.destination,
        "path": package.destination,
        "bytes": bytes,
        "size": map::human_size(bytes),
        "downloaded": bytes,
        "completeness": if failed { 0 } else { 100 },
        "download_time": 0,
        "postproc_time": 0,
        "stage_log": [],
        "url": "",
        "url_info": "",
        "report": "",
        "md5sum": "",
        "password": "",
        "action_line": "",
        "script_line": "",
        "series": "",
        "meta": serde_json::Value::Null,
        "loaded": false,
        "retry": 0,
    })
}

/// Why a package failed, in the one free-text field SABnzbd offers for it.
///
/// Left empty for anything that succeeded: a client shows this string to a user, and a
/// non-empty message on a completed import reads as a warning that is not there.
fn failure_message(package: &DownloadPackage) -> String {
    if package.state != PackageState::Failed {
        return String::new();
    }
    match package.extraction_result {
        Some(rd_core::ExtractionResult::Failed) => "Unpacking failed".to_owned(),
        _ => "Post-processing failed".to_owned(),
    }
}

async fn delete(state: &AppState, query: &SabQuery) -> Response {
    let Some(value) = query.value.as_deref() else {
        return error("nzo_id not found");
    };
    let ids: Vec<rd_core::PackageId> = if value.eq_ignore_ascii_case("all") {
        match state.database.list_packages().await {
            Ok(packages) => packages
                .iter()
                .filter(|package| map::is_history(package))
                .map(|package| package.id)
                .collect(),
            Err(_) => return error("history unavailable"),
        }
    } else {
        value.split(',').filter_map(map::package_id).collect()
    };
    if ids.is_empty() {
        return ok();
    }
    // `del_files=1` asks for the downloaded data to go as well. Removing the package rows
    // is the only thing this adapter does either way: the native removal already takes the
    // part files, and deleting finished files an automation client has just imported is a
    // destructive step that must stay a deliberate action in the application itself.
    // Forced: these clients name one item and expect it gone, and their protocols have no
    // way to carry back "it is still running, confirm first".
    match crate::package_handlers::remove_packages(state, ids, true).await {
        Ok(_) => ok(),
        Err(failure) => error(failure.message()),
    }
}

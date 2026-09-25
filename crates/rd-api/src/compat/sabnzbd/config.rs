//! Static and configuration modes: `version`, `auth`, `get_config`, `get_cats`, `status`.

use axum::response::Response;

use super::{REPORTED_VERSION, json};
use crate::AppState;

pub(crate) fn version() -> Response {
    json(serde_json::json!({ "version": REPORTED_VERSION }))
}

pub(crate) fn auth() -> Response {
    json(serde_json::json!({ "auth": "apikey" }))
}

/// The configuration subset an automation client reads.
///
/// Only the fields a client actually consults: the completed directory it will look in after
/// an import, and the category list it offers in its own settings. Nothing about servers,
/// credentials or scheduling is exposed here — this endpoint is reachable with an API key,
/// and a compatibility shim is no place to widen what such a key can read.
pub(crate) async fn get_config(state: &AppState) -> Response {
    let complete_dir = state
        .scheduler
        .downloads_directory()
        .to_string_lossy()
        .into_owned();
    let categories = category_names(state).await;
    json(serde_json::json!({
        "config": {
            "misc": {
                "complete_dir": complete_dir,
                "download_dir": complete_dir,
                "pre_check": false,
                "history_retention": "0",
                "enable_tv_sorting": false,
                "enable_movie_sorting": false,
                "enable_date_sorting": false,
            },
            "categories": categories
                .iter()
                .map(|name| serde_json::json!({ "name": name, "dir": "", "priority": 0 }))
                .collect::<Vec<_>>(),
        }
    }))
}

pub(crate) async fn get_cats(state: &AppState) -> Response {
    json(serde_json::json!({ "categories": category_names(state).await }))
}

/// A minimal `fullstatus`. Clients use it as a reachability probe.
pub(crate) async fn status(state: &AppState) -> Response {
    json(serde_json::json!({
        "status": {
            "version": REPORTED_VERSION,
            "paused": false,
            "categories": category_names(state).await,
        }
    }))
}

/// Category names with SABnzbd's implicit `*` default in front.
async fn category_names(state: &AppState) -> Vec<String> {
    let mut names = vec!["*".to_owned()];
    if let Ok(categories) = state.database.list_categories().await {
        names.extend(categories.into_iter().map(|category| category.name));
    }
    names
}

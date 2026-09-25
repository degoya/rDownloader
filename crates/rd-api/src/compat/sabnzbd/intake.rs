//! The `addfile` and `addurl` modes: getting an NZB into the queue.
//!
//! SABnzbd queues immediately, so these bypass the LinkGrabber review step that the native
//! upload keeps. That is the point of the adapter: an automation client has already decided
//! what it wants and waits for the job to appear in the queue.

use axum::{extract::Multipart, response::Response};

use super::{SabQuery, error, json, map};
use crate::AppState;

pub(crate) async fn add_file(
    state: &AppState,
    query: &SabQuery,
    multipart: Option<Multipart>,
) -> Response {
    let Some(mut multipart) = multipart else {
        return error("no nzb file");
    };
    let mut content: Option<axum::body::Bytes> = None;
    let mut name = query
        .nzbname
        .clone()
        .unwrap_or_else(|| "import.nzb".to_owned());
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                // SABnzbd clients name the part `nzbfile`; some send `name` alongside it.
                let field_name = field.name().unwrap_or_default().to_owned();
                let file_name = field.file_name().map(str::to_owned);
                match field_name.as_str() {
                    "nzbfile" | "file" => {
                        if query.nzbname.is_none()
                            && let Some(file_name) = file_name
                        {
                            name = file_name;
                        }
                        match field.bytes().await {
                            Ok(bytes) => content = Some(bytes),
                            Err(failure) => return error(&failure.to_string()),
                        }
                    }
                    "nzbname" => {
                        if let Ok(text) = field.text().await
                            && !text.trim().is_empty()
                        {
                            name = text.trim().to_owned();
                        }
                    }
                    _ => {}
                }
            }
            Ok(None) => break,
            Err(failure) => return error(&failure.to_string()),
        }
    }
    let Some(content) = content else {
        return error("no nzb file");
    };
    if content.len() > rd_collector::MAX_NZB_BYTES {
        return error("nzb too large");
    }
    queue_nzb(state, &content, &name, query).await
}

/// `addurl` fetches the NZB itself in SABnzbd. This adapter does not.
///
/// Following an arbitrary URL supplied through a compatibility endpoint would turn an API
/// key into a request-forgery primitive against the machine the service runs on. Clients
/// that get a refusal here fall back to `addfile`, which every one of them supports.
pub(crate) async fn add_url(_state: &AppState, _query: &SabQuery) -> Response {
    error("addurl is not supported; send the NZB with mode=addfile")
}

async fn queue_nzb(state: &AppState, content: &[u8], name: &str, query: &SabQuery) -> Response {
    let Ok(category_id) = resolve_category(state, query.cat.as_deref()).await else {
        return error("categories unavailable");
    };
    let import = match crate::handlers::store_nzb_import(
        state,
        content,
        name,
        category_id,
        // An external client handed this over; a rule can target it by `source = api`.
        rd_core::IngressSource::Api,
        None,
    )
    .await
    {
        Ok(import) => import,
        Err(failure) => return error(failure.message()),
    };
    // The category the import ended up with: for `*` or an unknown name that is whatever the
    // routing rules or the default category decided, and the files follow it.
    let destination =
        match crate::config_handlers::download_destination(state, import.category_id).await {
            Ok(destination) => {
                destination.unwrap_or_else(|| state.scheduler.downloads_directory().to_path_buf())
            }
            Err(failure) => return error(failure.message()),
        };
    match state
        .database
        // SABnzbd's paused priority is not mapped yet; this path always starts the job.
        .enqueue_nzb_import(
            import.id,
            destination,
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
    {
        Ok(package) => json(serde_json::json!({
            "status": true,
            "nzo_ids": [map::nzo_id(&package)],
        })),
        Err(failure) => error(&failure.to_string()),
    }
}

/// Maps a SABnzbd category name onto one of ours.
///
/// `*` and an unknown name both yield no category rather than failing: a client configured
/// with a category the user later renamed should keep importing. What the import then gets is
/// what any other uncategorised intake gets - the category of a matching routing rule, or the
/// default one - because `add_nzb_import` resolves it (RD-107-08).
async fn resolve_category(
    state: &AppState,
    name: Option<&str>,
) -> anyhow::Result<Option<rd_core::CategoryId>> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    if name == "*" {
        return Ok(None);
    }
    Ok(state
        .database
        .list_categories()
        .await?
        .into_iter()
        .find(|category| category.name.eq_ignore_ascii_case(name))
        .map(|category| category.id))
}

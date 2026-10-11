//! The aria2 methods this subset answers, each translated onto the native handlers.

use std::collections::HashMap;

use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::{
    REPORTED_VERSION, options,
    rpc::{self, Call, Request, RpcError},
    status::{self, List, PackageView},
};
use crate::{
    AppState,
    dto::{DownloadBulkAction, DownloadBulkFilter},
};

/// Every method this subset answers, as `system.listMethods` reports them.
///
/// `forcePause` and `forceRemove` are the spellings AriaNg sends from its toolbar; this service
/// has no gentler pause or removal than the one it has, so both are the plain ones. The
/// result, detail and option methods are the ones AriaNg calls from its stopped list, its task
/// page and its settings pages (RD-1240-28); `changeOption` and `changeGlobalOption` change
/// nothing (`options`).
pub(super) const METHODS: &[&str] = &[
    "aria2.addUri",
    "aria2.remove",
    "aria2.forceRemove",
    "aria2.pause",
    "aria2.forcePause",
    "aria2.unpause",
    "aria2.removeDownloadResult",
    "aria2.purgeDownloadResult",
    "aria2.tellStatus",
    "aria2.tellActive",
    "aria2.tellWaiting",
    "aria2.tellStopped",
    "aria2.getFiles",
    "aria2.getUris",
    "aria2.getPeers",
    "aria2.getServers",
    "aria2.getOption",
    "aria2.changeOption",
    "aria2.getGlobalOption",
    "aria2.changeGlobalOption",
    "aria2.getGlobalStat",
    "aria2.getVersion",
    "aria2.saveSession",
    "system.multicall",
    "system.listMethods",
];

/// Runs every call of an authorized request and answers it.
pub(super) async fn answer(state: &AppState, request: Request) -> Response {
    match request {
        Request::Single(call) => {
            let outcome = run(state, &call).await;
            rpc::single(call.id, &outcome)
        }
        Request::Batch(items) => {
            let mut answers = Vec::with_capacity(items.len());
            for item in items {
                answers.push(match item {
                    Ok(call) => {
                        let outcome = run(state, &call).await;
                        rpc::answer(call.id, &outcome)
                    }
                    Err(failure) => rpc::answer(failure.id, &Err(failure.error)),
                });
            }
            axum::Json(Value::Array(answers)).into_response()
        }
    }
}

async fn run(state: &AppState, call: &Call) -> Result<Value, RpcError> {
    if call.method == "system.multicall" {
        return multicall(state, &call.params).await;
    }
    method(state, &call.method, &call.params).await
}

/// `system.multicall`: each inner call answers `[result]` or its error struct, in order.
async fn multicall(state: &AppState, params: &[Value]) -> Result<Value, RpcError> {
    let entries = rpc::multicall_entries(params)
        .ok_or_else(|| RpcError::new("system.multicall expected a list of calls."))?;
    let mut results = Vec::with_capacity(entries.len());
    for entry in entries {
        let outcome = match entry {
            Some((name, _, params)) => method(state, &name, &params).await,
            None => Err(RpcError::new("Invalid call in system.multicall.")),
        };
        results.push(match outcome {
            Ok(value) => json!([value]),
            Err(error) => error.to_value(),
        });
    }
    Ok(Value::Array(results))
}

async fn method(state: &AppState, name: &str, params: &[Value]) -> Result<Value, RpcError> {
    match name {
        "aria2.addUri" => add_uri(state, params).await,
        "aria2.pause" | "aria2.forcePause" => act(state, params, DownloadBulkAction::Pause).await,
        "aria2.unpause" => act(state, params, DownloadBulkAction::Resume).await,
        "aria2.remove" | "aria2.forceRemove" => {
            act(state, params, DownloadBulkAction::Remove).await
        }
        "aria2.removeDownloadResult" => remove_result(state, params).await,
        "aria2.purgeDownloadResult" => purge_results(state).await,
        "aria2.tellStatus" => tell_status(state, params).await,
        "aria2.tellActive" => tell(state, List::Active, None, params.first()).await,
        "aria2.tellWaiting" => tell_window(state, List::Waiting, params).await,
        "aria2.tellStopped" => tell_window(state, List::Stopped, params).await,
        "aria2.getFiles" => options::files(&Snapshot::read(state).await?, params),
        "aria2.getUris" => options::uris(&Snapshot::read(state).await?, params),
        "aria2.getPeers" | "aria2.getServers" => {
            options::none_listed(&Snapshot::read(state).await?, params)
        }
        "aria2.getOption" => options::option(&Snapshot::read(state).await?, params),
        "aria2.changeOption" => options::change_option(&Snapshot::read(state).await?, params),
        "aria2.getGlobalOption" => options::global(state).await,
        "aria2.changeGlobalOption" => Ok(json!("OK")),
        "aria2.getGlobalStat" => global_stat(state).await,
        "aria2.getVersion" => Ok(json!({
            "version": REPORTED_VERSION,
            "enabledFeatures": ["BitTorrent", "HTTPS"],
        })),
        // Every change is in the store the moment it is made; there is no session to write.
        "aria2.saveSession" => Ok(json!("OK")),
        "system.listMethods" => Ok(json!(METHODS)),
        "system.multicall" => Err(RpcError::new("Recursive system.multicall forbidden.")),
        other => Err(RpcError::method_not_found(other)),
    }
}

/// `aria2.addUri([uris], options)`: a download of the first URI, queued without the
/// LinkGrabber review, as the other adapters queue.
///
/// aria2 reads the URIs as mirrors of one file, so only the first is queued; the rest would
/// fetch the same file again. It goes through the native `POST /api/v1/downloads` path with the
/// person's own reach, which is what the same token reaches there: HTTP(S) and magnet links.
/// Of the options, `out` names the file and `pause` adds it paused; `dir` is not honoured --
/// the routing rules decide where files land, as they do for SABnzbd and qBittorrent clients.
async fn add_uri(state: &AppState, params: &[Value]) -> Result<Value, RpcError> {
    let uri = params
        .first()
        .and_then(Value::as_array)
        .and_then(|uris| uris.iter().find_map(Value::as_str))
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
        .ok_or_else(|| RpcError::new("No URI to download."))?;
    let options = params.get(1).and_then(Value::as_object);
    let option = |key: &str| options.and_then(|options| options.get(key));
    let file_name = option("out")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    let paused = match option("pause") {
        Some(Value::Bool(paused)) => *paused,
        Some(Value::String(paused)) => paused.eq_ignore_ascii_case("true"),
        _ => false,
    };
    let request = crate::dto::CreateDownloadRequest {
        url: uri.to_owned(),
        package_name: None,
        file_name,
        category_id: None,
        account_id: None,
        proxy_profile_id: None,
        priority: None,
        paused,
    };
    let file = crate::download_handlers::create_download_inner(state, request)
        .await
        .map_err(|error| refused(error.message()))?;
    Ok(Value::String(status::gid(file.id)))
}

/// `pause`, `unpause` and `remove` of one GID; answers the GID, as aria2 does.
///
/// The removal is the native one, as the SABnzbd and qBittorrent adapters' delete: the file
/// leaves the queue, part data with it. A finished file's payload stays on disk.
async fn act(
    state: &AppState,
    params: &[Value],
    action: DownloadBulkAction,
) -> Result<Value, RpcError> {
    let wanted = params.first().and_then(Value::as_str).unwrap_or_default();
    let downloads = state.database.list_downloads().await.map_err(unavailable)?;
    let id = status::find(&downloads, wanted)
        .ok_or_else(|| not_found(wanted))?
        .id;
    apply(state, action, id).await?;
    Ok(Value::String(status::gid(id)))
}

/// One native action on one download.
async fn apply(
    state: &AppState,
    action: DownloadBulkAction,
    id: rd_core::DownloadId,
) -> Result<(), RpcError> {
    match crate::download_handlers::apply_download_action(state, action, vec![id]).await {
        // A refusal is reported inside the answer, not as an error: nothing done is a failure
        // the client has to see, or it strikes the item off as handled.
        Ok(result) if result.affected == 0 => Err(refused(
            result
                .errors
                .first()
                .map_or("The action could not be applied", String::as_str),
        )),
        Ok(_) => Ok(()),
        Err(error) => Err(refused(error.message())),
    }
}

/// `aria2.removeDownloadResult(gid)`: a finished, failed or removed download leaves the list,
/// as "Remove Task" on AriaNg's stopped list asks. The native removal, so a finished file's
/// payload stays on disk. A download still in the active or waiting list is refused in
/// aria2's words; `remove` is the call for it.
async fn remove_result(state: &AppState, params: &[Value]) -> Result<Value, RpcError> {
    let wanted = params.first().and_then(Value::as_str).unwrap_or_default();
    let downloads = state.database.list_downloads().await.map_err(unavailable)?;
    let download = status::find(&downloads, wanted).ok_or_else(|| not_found(wanted))?;
    if !options::is_result(download) {
        return Err(RpcError::new(format!(
            "Could not remove download result of GID#{wanted}"
        )));
    }
    apply(state, DownloadBulkAction::Remove, download.id).await?;
    Ok(json!("OK"))
}

/// `aria2.purgeDownloadResult()`: every download of the stopped list leaves it, as AriaNg's
/// "Clear Stopped Tasks" asks. A file the native removal refuses is reported, not dropped.
async fn purge_results(state: &AppState) -> Result<Value, RpcError> {
    let filter = DownloadBulkFilter {
        states: vec![
            rd_core::DownloadState::Completed,
            rd_core::DownloadState::Failed,
            rd_core::DownloadState::Cancelled,
        ],
        package_id: None,
    };
    let result = crate::download_handlers::apply_download_action_to(
        state,
        DownloadBulkAction::Remove,
        Vec::new(),
        Some(filter),
    )
    .await
    .map_err(|error| refused(error.message()))?;
    match result.errors.first() {
        Some(error) => Err(refused(error)),
        None => Ok(json!("OK")),
    }
}

/// What the list methods read: every download, its package and its current rate.
pub(super) struct Snapshot {
    downloads: Vec<rd_core::DownloadFile>,
    packages: HashMap<rd_core::PackageId, rd_core::DownloadPackage>,
    rates: HashMap<rd_core::DownloadId, u64>,
}

impl Snapshot {
    pub(super) async fn read(state: &AppState) -> Result<Self, RpcError> {
        let downloads = state.database.list_downloads().await.map_err(unavailable)?;
        let packages = state
            .database
            .list_packages()
            .await
            .map_err(unavailable)?
            .into_iter()
            .map(|package| (package.id, package))
            .collect();
        // Only what is moving: the scheduler's smoothed rate outlives a finished transfer by a
        // few seconds, as the native rates route filters too.
        let moving: std::collections::HashSet<rd_core::DownloadId> = downloads
            .iter()
            .filter(|download| download.state == rd_core::DownloadState::Downloading)
            .map(|download| download.id)
            .collect();
        let rates = state
            .scheduler
            .transfer_rates()
            .into_iter()
            .filter(|(id, _)| moving.contains(id))
            .collect();
        Ok(Self {
            downloads,
            packages,
            rates,
        })
    }

    /// The download the first parameter's GID names.
    pub(super) fn find(&self, params: &[Value]) -> Result<&rd_core::DownloadFile, RpcError> {
        let wanted = params.first().and_then(Value::as_str).unwrap_or_default();
        status::find(&self.downloads, wanted).ok_or_else(|| not_found(wanted))
    }

    pub(super) fn entry(&self, download: &rd_core::DownloadFile) -> serde_json::Map<String, Value> {
        let package = self
            .packages
            .get(&download.package_id)
            .map(|package| PackageView {
                name: &package.name,
                destination: &package.destination,
            });
        let rate = self.rates.get(&download.id).copied().unwrap_or_default();
        status::entry(download, package.as_ref(), rate)
    }

    fn in_list(&self, list: List) -> Vec<&rd_core::DownloadFile> {
        self.downloads
            .iter()
            .filter(|download| status::list_of(download.state) == list)
            .collect()
    }
}

async fn tell_status(state: &AppState, params: &[Value]) -> Result<Value, RpcError> {
    let snapshot = Snapshot::read(state).await?;
    let download = snapshot.find(params)?;
    Ok(status::project(snapshot.entry(download), params.get(1)))
}

/// `tellWaiting(offset, num, keys)` and `tellStopped(offset, num, keys)`.
async fn tell_window(state: &AppState, list: List, params: &[Value]) -> Result<Value, RpcError> {
    let number = |index: usize| {
        params
            .get(index)
            .and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
            })
            .ok_or_else(|| RpcError::new("offset and num must be integers."))
    };
    let window = (number(0)?, number(1)?);
    tell(state, list, Some(window), params.get(2)).await
}

async fn tell(
    state: &AppState,
    list: List,
    window: Option<(i64, i64)>,
    keys: Option<&Value>,
) -> Result<Value, RpcError> {
    let snapshot = Snapshot::read(state).await?;
    let mut members = snapshot.in_list(list);
    if let Some((offset, num)) = window {
        members = status::window(&members, offset, num);
    }
    Ok(Value::Array(
        members
            .into_iter()
            .map(|download| status::project(snapshot.entry(download), keys))
            .collect(),
    ))
}

async fn global_stat(state: &AppState) -> Result<Value, RpcError> {
    let snapshot = Snapshot::read(state).await?;
    let count = |list: List| snapshot.in_list(list).len().to_string();
    let speed = snapshot
        .rates
        .values()
        .copied()
        .fold(0_u64, u64::saturating_add);
    let stopped = count(List::Stopped);
    Ok(json!({
        "downloadSpeed": speed.to_string(),
        "uploadSpeed": "0",
        "numActive": count(List::Active),
        "numWaiting": count(List::Waiting),
        "numStopped": stopped,
        "numStoppedTotal": stopped,
    }))
}

fn not_found(gid: &str) -> RpcError {
    RpcError::new(format!("GID {gid} is not found"))
}

/// A refusal of the native handler, its message redacted: it can quote a signed link, and this
/// answer ends up in a front end's log.
fn refused(message: &str) -> RpcError {
    RpcError::new(rd_core::redact_text(message))
}

/// A store that could not be read: said as this service's fault, with the cause in the log.
/// An empty list would be a statement -- "nothing is queued" -- and a front end acts on it.
fn unavailable(error: anyhow::Error) -> RpcError {
    tracing::warn!(
        error = %format!("{error:#}"),
        "the aria2 adapter could not read the store"
    );
    RpcError::new("The download list could not be read; try again later.")
}

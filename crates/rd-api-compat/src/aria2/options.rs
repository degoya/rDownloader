//! The detail and option methods AriaNg calls beside the queue (RD-1240-28): a task's files,
//! links and peers, its options and the global ones.
//!
//! The options are read, never written. `changeOption` and `changeGlobalOption` answer `OK`
//! without effect: AriaNg sends them from its settings pages, and every value they could set is
//! this service's own setting, edited on its own settings page. A refusal would put an error
//! dialog in front of each field AriaNg shows, for a change this adapter does not take either way.

use serde_json::{Value, json};

use super::{methods::Snapshot, rpc::RpcError, status};
use crate::AppState;

/// `aria2.getFiles(gid)`: the `files` array of the status struct.
pub(super) fn files(snapshot: &Snapshot, params: &[Value]) -> Result<Value, RpcError> {
    let mut entry = snapshot.entry(snapshot.find(params)?);
    Ok(entry.remove("files").unwrap_or_else(|| json!([])))
}

/// `aria2.getUris(gid)`: the links of the download's one file.
pub(super) fn uris(snapshot: &Snapshot, params: &[Value]) -> Result<Value, RpcError> {
    let files = files(snapshot, params)?;
    Ok(files[0].get("uris").cloned().unwrap_or_else(|| json!([])))
}

/// `aria2.getPeers(gid)` and `aria2.getServers(gid)`: none listed. The torrent engine's peers
/// and the HTTP engine's connections are not this subset's; an empty list is what aria2 answers
/// for a download without any.
pub(super) fn none_listed(snapshot: &Snapshot, params: &[Value]) -> Result<Value, RpcError> {
    snapshot.find(params)?;
    Ok(json!([]))
}

/// `aria2.getOption(gid)`: where the download lands and under which name.
pub(super) fn option(snapshot: &Snapshot, params: &[Value]) -> Result<Value, RpcError> {
    let download = snapshot.find(params)?;
    let entry = snapshot.entry(download);
    Ok(json!({
        "dir": entry.get("dir").cloned().unwrap_or_else(|| json!("")),
        "out": download.file_name,
    }))
}

/// `aria2.changeOption(gid, options)`: `OK` for a download that exists, nothing changed.
pub(super) fn change_option(snapshot: &Snapshot, params: &[Value]) -> Result<Value, RpcError> {
    snapshot.find(params)?;
    Ok(json!("OK"))
}

/// `aria2.getGlobalOption()`: the settings an aria2 client knows by aria2's names, as aria2
/// sends them -- every value a string, `0` for no limit.
pub(super) async fn global(state: &AppState) -> Result<Value, RpcError> {
    let settings = rd_api_core::settings_store::read_settings(state)
        .await
        .map_err(|error| {
            tracing::warn!(
                error = error.message(),
                "the aria2 adapter could not read the settings"
            );
            RpcError::new("The settings could not be read; try again later.")
        })?;
    let limit =
        |limit: Option<rd_core::ByteCount>| limit.map_or(0, rd_core::ByteCount::get).to_string();
    Ok(json!({
        "dir": state.scheduler.downloads_directory().to_string_lossy(),
        "max-concurrent-downloads": settings.max_active_files.to_string(),
        "max-connection-per-server": settings.max_connections_per_host.to_string(),
        "split": settings.max_chunks_per_file.to_string(),
        "max-overall-download-limit": limit(settings.speed_limit_bytes_per_second),
        "max-overall-upload-limit": limit(settings.upload_limit_bytes_per_second),
        "continue": "true",
    }))
}

/// Whether a download is in aria2's stopped list, the only one `removeDownloadResult` takes.
pub(super) fn is_result(download: &rd_core::DownloadFile) -> bool {
    status::list_of(download.state) == status::List::Stopped
}

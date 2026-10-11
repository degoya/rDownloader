//! What a download looks like to an aria2 client: its GID, its status and the status struct
//! of `aria2.tellStatus`, and the windows `tellWaiting` and `tellStopped` cut from a list.

use rd_core::{DownloadFile, DownloadKind, DownloadState};
use serde_json::{Map, Value, json};

/// aria2's GID of a download: the low 64 bits of its id as 16 hex digits.
///
/// aria2 documents a GID as exactly that, and clients store and compare it as such, so the
/// 128-bit id does not go out whole. The low half of a UUIDv7 is its counter and random part,
/// so two downloads never share it in practice, and the id is found again by comparing every
/// download's GID ([`find`]) rather than kept in a table of its own.
pub(super) fn gid(id: rd_core::DownloadId) -> String {
    let simple: String = id
        .to_string()
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect();
    simple
        .get(simple.len().saturating_sub(16)..)
        .unwrap_or_default()
        .to_owned()
}

/// The download a client's GID names. Case-insensitive, surrounding space ignored.
pub(super) fn find<'a>(downloads: &'a [DownloadFile], wanted: &str) -> Option<&'a DownloadFile> {
    let wanted = wanted.trim().to_ascii_lowercase();
    if wanted.len() != 16 {
        return None;
    }
    downloads.iter().find(|download| gid(download.id) == wanted)
}

/// aria2's status of a download: `active`, `waiting`, `paused`, `error`, `complete` or
/// `removed`.
///
/// Verifying, repairing, unpacking and seeding are still work on the download, so `active`,
/// as aria2 reports a seeding torrent; a file waiting for its turn, a retry, its mirror or a
/// switched-off service is `waiting`. A cancelled file is aria2's `removed`.
pub(super) fn status_of(state: DownloadState) -> &'static str {
    match state {
        DownloadState::Resolving
        | DownloadState::Downloading
        | DownloadState::Verifying
        | DownloadState::Repairing
        | DownloadState::Extracting
        | DownloadState::Seeding => "active",
        DownloadState::Queued
        | DownloadState::RetryWait
        | DownloadState::Skipped
        | DownloadState::Blocked => "waiting",
        DownloadState::Paused => "paused",
        DownloadState::Failed => "error",
        DownloadState::Cancelled => "removed",
        DownloadState::Completed => "complete",
    }
}

/// Which of aria2's three lists a status belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum List {
    Active,
    /// `tellWaiting`: waiting and paused alike, as in aria2.
    Waiting,
    Stopped,
}

pub(super) fn list_of(state: DownloadState) -> List {
    match status_of(state) {
        "active" => List::Active,
        "waiting" | "paused" => List::Waiting,
        _ => List::Stopped,
    }
}

/// The package a download belongs to, as far as the status struct needs it.
pub(super) struct PackageView<'a> {
    pub name: &'a str,
    pub destination: &'a str,
}

/// The status struct of `aria2.tellStatus` and the list methods.
///
/// Numbers are decimal strings, as aria2 sends them. The speed is the scheduler's current rate
/// of the download; upload figures are zero, as the qBittorrent adapter reports them.
pub(super) fn entry(
    download: &DownloadFile,
    package: Option<&PackageView<'_>>,
    bytes_per_second: u64,
) -> Map<String, Value> {
    let total = download.total_bytes.map_or(0, rd_core::ByteCount::get);
    let done = download.committed_bytes.get();
    let status = status_of(download.state);
    let dir = package.map_or("", |package| package.destination);
    let path = if dir.is_empty() {
        download.file_name.clone()
    } else {
        std::path::Path::new(dir)
            .join(&download.file_name)
            .to_string_lossy()
            .into_owned()
    };
    let torrent = download.kind == DownloadKind::Torrent;
    // A torrent's magnet is no URI the payload is fetched from; aria2 lists none for it either.
    let uris = if torrent {
        Vec::new()
    } else {
        vec![json!({ "uri": download.source.as_str(), "status": "used" })]
    };
    let connections = if bytes_per_second > 0 { "1" } else { "0" };
    let error_code = if status == "error" { "1" } else { "0" };
    let mut entry = json!({
        "gid": gid(download.id),
        "status": status,
        "totalLength": total.to_string(),
        "completedLength": done.to_string(),
        "uploadLength": "0",
        "downloadSpeed": bytes_per_second.to_string(),
        "uploadSpeed": "0",
        "connections": connections,
        "errorCode": error_code,
        "dir": dir,
        "files": [{
            "index": "1",
            "path": path,
            "length": total.to_string(),
            "completedLength": done.to_string(),
            "selected": "true",
            "uris": uris,
        }],
    });
    let Some(map) = entry.as_object_mut() else {
        return Map::new();
    };
    if status == "error"
        && let Some(failure) = &download.last_error
    {
        // A failure can quote a signed link; the client's log is no place for its signature.
        map.insert(
            "errorMessage".to_owned(),
            Value::String(rd_core::redact_text(&failure.message)),
        );
    }
    if torrent {
        // Front ends name a torrent by `bittorrent.info.name` and show `seeder`.
        let name = package.map_or(download.file_name.as_str(), |package| package.name);
        map.insert("bittorrent".to_owned(), json!({ "info": { "name": name } }));
        map.insert(
            "seeder".to_owned(),
            Value::String((download.state == DownloadState::Seeding).to_string()),
        );
    }
    std::mem::take(map)
}

/// The fields a client asked for with `keys`; all of them when it named none.
///
/// A key aria2 knows and this subset does not fill is left out, as aria2 leaves out a key that
/// does not apply.
pub(super) fn project(mut entry: Map<String, Value>, keys: Option<&Value>) -> Value {
    if let Some(Value::Array(keys)) = keys
        && !keys.is_empty()
    {
        entry.retain(|key, _| keys.iter().any(|wanted| wanted.as_str() == Some(key)));
    }
    Value::Object(entry)
}

/// The part of a list `tellWaiting` and `tellStopped` answer for `offset` and `num`.
///
/// aria2's reading: a non-negative offset counts from the front and the window runs forward;
/// a negative one counts from the back, `-1` being the last entry, and the window runs
/// backwards from there, so its entries come in reverse order.
pub(super) fn window<T: Clone>(items: &[T], offset: i64, num: i64) -> Vec<T> {
    let Ok(num) = usize::try_from(num) else {
        return Vec::new();
    };
    if let Ok(offset) = usize::try_from(offset) {
        return items.iter().skip(offset).take(num).cloned().collect();
    }
    let back = usize::try_from(offset.unsigned_abs()).unwrap_or(usize::MAX);
    let Some(start) = items.len().checked_sub(back) else {
        return Vec::new();
    };
    items[..=start].iter().rev().take(num).cloned().collect()
}

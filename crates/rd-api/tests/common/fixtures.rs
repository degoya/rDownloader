//! Engine state a test writes to disk instead of producing it.

/// Writes librqbit's persisted session below a harness directory, holding one paused torrent
/// the way the engine leaves it after a restart (RD-120-68).
///
/// No session is built in these tests, so this file is the whole engine state: a removal
/// that reaches the engine strikes the entry from it.
pub fn persist_torrent_session(directory: &std::path::Path, info_hash: &str) {
    let folder = directory.join("torrent-session");
    std::fs::create_dir_all(&folder).expect("session folder");
    let session = serde_json::json!({
        "torrents": {
            "0": {
                "info_hash": info_hash,
                "trackers": [],
                "output_folder": directory.join("downloads"),
                "only_files": null,
                "is_paused": true
            }
        }
    });
    std::fs::write(
        folder.join("session.json"),
        serde_json::to_vec(&session).expect("json"),
    )
    .expect("session.json");
}

/// The info hashes librqbit's persisted session still holds.
pub fn persisted_torrents(directory: &std::path::Path) -> Vec<String> {
    let bytes = std::fs::read(directory.join("torrent-session").join("session.json"))
        .expect("session.json");
    let session: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    session["torrents"]
        .as_object()
        .map(|torrents| {
            torrents
                .values()
                .filter_map(|torrent| torrent["info_hash"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

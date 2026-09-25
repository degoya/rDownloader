//! A removed torrent stays removed (RD-120-68).

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use rd_core::{DownloadId, DownloadKind, DownloadState, PackageId};
use serde_json::json;

use super::{drop_orphans, prune_persisted, session_folder};
use crate::{TorrentService, parse_torrent};

/// A directory below the system temp dir, removed again when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rd-torrent-forget-{}", DownloadId::new()));
        std::fs::create_dir_all(&path).expect("scratch dir");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bstr(value: &str) -> Vec<u8> {
    let mut out = format!("{}:", value.len()).into_bytes();
    out.extend_from_slice(value.as_bytes());
    out
}

/// A one-byte single-file torrent; the name makes the info hash differ.
fn torrent(name: &str) -> Vec<u8> {
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("announce"));
    bytes.extend(bstr("http://tracker.example/announce"));
    bytes.extend(bstr("info"));
    bytes.push(b'd');
    bytes.extend(bstr("length"));
    bytes.extend_from_slice(b"i1e");
    bytes.extend(bstr("name"));
    bytes.extend(bstr(name));
    bytes.extend(bstr("piece length"));
    bytes.extend_from_slice(b"i16384e");
    bytes.extend(bstr("pieces"));
    bytes.extend_from_slice(b"20:");
    bytes.extend_from_slice(&[7_u8; 20]);
    bytes.push(b'e');
    bytes.push(b'e');
    bytes
}

/// One paused entry the way librqbit persists it, with its `.torrent` beside the list.
fn persist(folder: &Path, entries: &[(&str, &[u8], &Path)]) {
    std::fs::create_dir_all(folder).expect("session folder");
    let mut torrents = serde_json::Map::new();
    for (index, (hash, bytes, output)) in entries.iter().enumerate() {
        torrents.insert(
            index.to_string(),
            json!({
                "info_hash": hash,
                "trackers": [],
                "output_folder": output,
                "only_files": null,
                "is_paused": true,
            }),
        );
        std::fs::write(folder.join(format!("{hash}.torrent")), bytes).expect("torrent bytes");
        std::fs::write(folder.join(format!("{hash}.bitv")), [0_u8]).expect("bitv");
    }
    std::fs::write(
        folder.join("session.json"),
        serde_json::to_vec(&json!({ "torrents": torrents })).expect("json"),
    )
    .expect("session.json");
}

fn persisted_hashes(folder: &Path) -> Vec<String> {
    let bytes = std::fs::read(folder.join("session.json")).expect("session.json");
    let document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    document["torrents"]
        .as_object()
        .expect("torrents")
        .values()
        .filter_map(|torrent| torrent["info_hash"].as_str().map(str::to_owned))
        .collect()
}

/// A service over a fresh database holding one paused torrent row for `magnet_hash`.
async fn service_with_row(scratch: &Path, magnet_hash: &str) -> (TorrentService, DownloadId) {
    let database = rd_db::Database::open(scratch.join("torrent.sqlite3"))
        .await
        .expect("database");
    let package = database
        .create_package(rd_db::NewPackage {
            id: PackageId::new(),
            name: "kept".to_owned(),
            destination: scratch.join("downloads").display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let id = DownloadId::new();
    database
        .create_download(rd_db::NewDownload {
            id,
            package_id: package.id,
            source: format!("magnet:?xt=urn:btih:{magnet_hash}")
                .parse()
                .expect("magnet"),
            file_name: "kept.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Paused,
            kind: DownloadKind::Torrent,
            media: None,
            remote_credential_id: None,
            replay: None,
            secret_fragment: None,
            mirror_group: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("download");
    let settings = crate::shared_settings(&database).await.expect("settings");
    let service = TorrentService::start(
        database,
        settings,
        scratch.to_path_buf(),
        scratch.join("downloads"),
    );
    (service, id)
}

#[tokio::test]
async fn pruning_strikes_only_the_rejected_entries_and_their_files() {
    let scratch = Scratch::new();
    let folder = scratch.0.join("session");
    let output = scratch.0.join("out");
    let orphan = "aa".repeat(20);
    let kept = "bb".repeat(20);
    persist(
        &folder,
        &[
            (orphan.as_str(), &b"x"[..], output.as_path()),
            (kept.as_str(), &b"y"[..], output.as_path()),
        ],
    );

    let removed = prune_persisted(&folder, |hash| hash == kept)
        .await
        .expect("prune");

    assert_eq!(removed, vec![orphan.clone()]);
    assert_eq!(persisted_hashes(&folder), vec![kept.clone()]);
    assert!(!folder.join(format!("{orphan}.torrent")).exists());
    assert!(!folder.join(format!("{orphan}.bitv")).exists());
    assert!(folder.join(format!("{kept}.torrent")).exists());
    // Nothing to strike leaves the file alone, and a missing list is no error.
    assert!(
        prune_persisted(&folder, |_| true)
            .await
            .expect("noop")
            .is_empty()
    );
    assert!(
        prune_persisted(&scratch.0.join("absent"), |_| false)
            .await
            .expect("absent")
            .is_empty()
    );
}

/// The owner's case: a session holding a torrent whose row is gone. The orphan is struck
/// before librqbit reads the list, so its folder is never created — while the torrent that
/// still has a row is restored and does get its file, which proves the setup would create
/// one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_orphan_is_dropped_before_librqbit_can_create_its_files() {
    let scratch = Scratch::new();
    let kept_bytes = torrent("kept.bin");
    let orphan_bytes = torrent("orphan.bin");
    let kept = parse_torrent(&kept_bytes).expect("kept").info_hash;
    let orphan = parse_torrent(&orphan_bytes).expect("orphan").info_hash;
    let kept_output = scratch.0.join("kept-output");
    let orphan_output = scratch.0.join("orphan-output");
    let folder = session_folder(&scratch.0);
    persist(
        &folder,
        &[
            (
                orphan.as_str(),
                orphan_bytes.as_slice(),
                orphan_output.as_path(),
            ),
            (kept.as_str(), kept_bytes.as_slice(), kept_output.as_path()),
        ],
    );
    let (service, _) = service_with_row(&scratch.0, &kept).await;

    drop_orphans(&service.inner).await;
    assert_eq!(persisted_hashes(&folder), vec![kept.clone()]);

    // The same persistence the service uses, without the network.
    let session = librqbit::Session::new_with_opts(
        scratch.0.join("downloads"),
        librqbit::SessionOptions {
            dht: None,
            listen: None,
            disable_trackers: true,
            disable_local_service_discovery: true,
            fastresume: true,
            persistence: Some(librqbit::SessionPersistenceConfig::Json {
                folder: Some(folder.clone()),
            }),
            ..Default::default()
        },
    )
    .await
    .expect("session");
    let kept_file = kept_output.join("kept.bin");
    for _ in 0..100 {
        if kept_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(kept_file.exists(), "the restored torrent must get its file");
    assert!(
        !orphan_output.exists(),
        "the orphan must never be initialised"
    );
    assert_eq!(session.with_torrents(|torrents| torrents.count()), 1);
    session.cancellation_token().cancel();
    service.shutdown();
}

/// After a restart the registry is empty; the hash comes from the row, and with no session
/// built yet the torrent is struck from the persisted list instead.
#[tokio::test]
async fn forget_finds_the_torrent_after_a_restart() {
    let scratch = Scratch::new();
    let bytes = torrent("kept.bin");
    let hash = parse_torrent(&bytes).expect("torrent").info_hash;
    let folder = session_folder(&scratch.0);
    let output = scratch.0.join("out");
    persist(
        &folder,
        &[(hash.as_str(), bytes.as_slice(), output.as_path())],
    );
    let (service, id) = service_with_row(&scratch.0, &hash).await;

    let location = service.locate(id).await;
    assert_eq!(location.info_hash(), Some(hash.as_str()));
    service.forget_located(location).await;

    assert!(persisted_hashes(&folder).is_empty());
    assert!(!folder.join(format!("{hash}.torrent")).exists());
    service.shutdown();
}

/// A row the queue does not hold never touches the engine.
#[tokio::test]
async fn an_unknown_row_locates_nothing() {
    let scratch = Scratch::new();
    let (service, _) = service_with_row(&scratch.0, &"cc".repeat(20)).await;
    assert_eq!(service.locate(DownloadId::new()).await.info_hash(), None);
    service.shutdown();
}

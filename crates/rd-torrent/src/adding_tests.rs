//! Two torrents added at once each get their own handle (RD-1240-28).

use std::path::PathBuf;

use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse};
use rd_core::CandidateId;

use super::is_foreign;
use crate::{TorrentService, parse_torrent};

/// A directory below the system temp dir, removed again when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rd-torrent-adding-{}", CandidateId::new()));
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

/// A one-byte single-file torrent without a tracker; the name makes the info hash differ.
fn torrent(name: &str) -> Vec<u8> {
    let mut bytes = vec![b'd'];
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

/// What the runner does when the queue starts with several torrents and no session yet: every
/// add races the others into a session that was just built. Before RD-1240-28 the second add
/// answered "already managed" with the first torrent's handle.
#[tokio::test]
async fn torrents_added_at_once_to_a_new_session_keep_their_own_handles() {
    let scratch = Scratch::new();
    let database = rd_db::Database::open(scratch.0.join("torrent.sqlite3"))
        .await
        .expect("database");
    let settings = crate::shared_settings(&database).await.expect("settings");
    let service = TorrentService::start(
        database,
        settings,
        scratch.0.clone(),
        scratch.0.join("downloads"),
    );
    let session = service.session().await.expect("session");
    let names = ["first", "second", "third", "fourth"];
    let adds = names.map(|name| {
        let service = service.clone();
        let session = session.clone();
        let output = scratch.0.join("downloads").join(name);
        tokio::spawn(async move {
            let options = AddTorrentOptions {
                paused: true,
                overwrite: true,
                output_folder: Some(output.to_string_lossy().into_owned()),
                ..Default::default()
            };
            service
                .add_to(&session, AddTorrent::from_bytes(torrent(name)), options)
                .await
                .map(|response| match response {
                    AddTorrentResponse::Added(id, handle) => (id, handle.info_hash().as_string()),
                    other => panic!(
                        "{name}: not added as its own torrent: {:?}",
                        other
                            .into_handle()
                            .map(|handle| handle.info_hash().as_string())
                    ),
                })
        })
    });
    let mut ids = std::collections::HashSet::new();
    for (name, add) in names.into_iter().zip(adds) {
        let (id, hash) = add.await.expect("task").expect("add");
        let expected = parse_torrent(&torrent(name)).expect("fixture").info_hash;
        assert_eq!(hash, expected, "{name}");
        assert!(ids.insert(id), "{name} shares id {id}");
    }
}

#[test]
fn another_torrent_s_handle_is_foreign_only_against_a_known_hash() {
    let hash = "5bc6b46b9d9bab54fba6c9dc7aae44f0f46c4949";
    assert!(!is_foreign(hash, Some(hash)));
    assert!(!is_foreign(hash, Some(&hash.to_ascii_uppercase())));
    assert!(is_foreign(
        hash,
        Some("9faddfe6b293431311d97c97901610e8b17a33b3")
    ));
    assert!(!is_foreign(hash, None));
}

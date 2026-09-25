//! The torrent a check read is kept for its download, and expires (RD-130-18).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::SystemTime,
};

use rd_core::CandidateId;

use super::PREFETCH_TTL;
use crate::{TorrentService, parse_torrent};

/// A directory below the system temp dir, removed again when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rd-torrent-prefetch-{}", CandidateId::new()));
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

async fn service(scratch: &Path) -> TorrentService {
    let database = rd_db::Database::open(scratch.join("torrent.sqlite3"))
        .await
        .expect("database");
    let settings = crate::shared_settings(&database).await.expect("settings");
    TorrentService::start(
        database,
        settings,
        scratch.to_path_buf(),
        scratch.join("downloads"),
    )
}

/// Sets a kept file's modification time as if it had been written `age` ago.
fn age(path: &Path, age: std::time::Duration) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open kept file");
    file.set_modified(SystemTime::now() - age)
        .expect("set modification time");
}

#[tokio::test]
async fn a_kept_torrent_is_stored_by_info_hash_for_its_candidate() {
    let scratch = Scratch::new();
    let service = service(&scratch.0).await;
    let bytes = torrent("release");
    let hash = parse_torrent(&bytes).expect("fixture").info_hash;
    let candidate = CandidateId::new();

    service
        .keep_prefetched(candidate, &bytes)
        .await
        .expect("keep");
    let stored = service
        .promote_prefetched(candidate, &hash)
        .await
        .expect("promote")
        .expect("a kept torrent is promoted");

    assert_eq!(
        stored.file_name().and_then(|name| name.to_str()),
        Some(format!("{hash}.torrent").as_str())
    );
    assert_eq!(std::fs::read(&stored).expect("stored file"), bytes);
    // Another candidate has nothing kept, and a hash the candidate was not reviewed with
    // is not handed out.
    assert!(
        service
            .promote_prefetched(CandidateId::new(), &hash)
            .await
            .expect("promote")
            .is_none()
    );
    let other = parse_torrent(&torrent("other")).expect("other").info_hash;
    assert!(
        service
            .promote_prefetched(candidate, &other)
            .await
            .expect("promote")
            .is_none()
    );
}

#[tokio::test]
async fn an_expired_torrent_is_not_reused_and_is_removed() {
    let scratch = Scratch::new();
    let service = service(&scratch.0).await;
    let bytes = torrent("release");
    let hash = parse_torrent(&bytes).expect("fixture").info_hash;
    let candidate = CandidateId::new();
    service
        .keep_prefetched(candidate, &bytes)
        .await
        .expect("keep");
    let kept = service.prefetch_path(candidate);
    age(&kept, PREFETCH_TTL + std::time::Duration::from_secs(60));

    assert!(
        service
            .promote_prefetched(candidate, &hash)
            .await
            .expect("promote")
            .is_none()
    );
    assert!(!kept.exists(), "the expired file is removed");
}

#[tokio::test]
async fn pruning_keeps_only_fresh_files_of_open_candidates() {
    let scratch = Scratch::new();
    let service = service(&scratch.0).await;
    let bytes = torrent("release");
    let (open, stale, gone) = (CandidateId::new(), CandidateId::new(), CandidateId::new());
    for candidate in [open, stale, gone] {
        service
            .keep_prefetched(candidate, &bytes)
            .await
            .expect("keep");
    }
    age(
        &service.prefetch_path(stale),
        PREFETCH_TTL + std::time::Duration::from_secs(60),
    );

    service
        .prune_prefetched(&HashSet::from([open, stale]))
        .await;

    assert!(service.prefetch_path(open).exists());
    assert!(!service.prefetch_path(stale).exists(), "expired");
    assert!(!service.prefetch_path(gone).exists(), "candidate removed");
}

//! Moving a torrent's files and hashing them again (RD-1100-10).
//!
//! What is exercised is the files, the queue row and the registry, not the swarm: the sessions
//! are offline, as in `seeding_crash_tests`, and the torrent's one piece matches nothing, which
//! is exactly what a recheck of damaged data finds.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rd_core::{DownloadId, DownloadKind, DownloadState, PackageId};

use super::{relative_path, remove_empty_folders};
use crate::{
    SharedTorrentSettings, TorrentService,
    registry::TorrentPhase,
    session::{SessionConfig, SessionSlot},
};

/// A directory below the system temp dir, removed again when the test ends.
pub(super) struct Scratch(pub(super) PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rd-torrent-move-{}", DownloadId::new()));
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

/// One entry of a multi-file torrent's `files` list, one byte long.
fn file_entry(path: &[&str]) -> Vec<u8> {
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("length"));
    bytes.extend_from_slice(b"i1e");
    bytes.extend(bstr("path"));
    bytes.push(b'l');
    for component in path {
        bytes.extend(bstr(component));
    }
    bytes.push(b'e');
    bytes.push(b'e');
    bytes
}

/// A torrent of two one-byte files, `a.bin` and `sub/b.bin`, whose one piece matches nothing.
fn torrent() -> Vec<u8> {
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("announce"));
    bytes.extend(bstr("http://tracker.example/announce"));
    bytes.extend(bstr("info"));
    bytes.push(b'd');
    bytes.extend(bstr("files"));
    bytes.push(b'l');
    bytes.extend(file_entry(&["a.bin"]));
    bytes.extend(file_entry(&["sub", "b.bin"]));
    bytes.push(b'e');
    bytes.extend(bstr("name"));
    bytes.extend(bstr("Release"));
    bytes.extend(bstr("piece length"));
    bytes.extend_from_slice(b"i16384e");
    bytes.extend(bstr("pieces"));
    bytes.extend_from_slice(b"20:");
    bytes.extend_from_slice(&[9_u8; 20]);
    bytes.push(b'e');
    bytes.push(b'e');
    bytes
}

/// Gives `service` a session that never reaches the network, before anything asks for one.
async fn go_offline(service: &TorrentService, output: PathBuf) -> Arc<librqbit::Session> {
    let session = librqbit::Session::new_with_opts(
        output,
        librqbit::SessionOptions {
            dht: None,
            listen: None,
            disable_trackers: true,
            disable_local_service_discovery: true,
            ..Default::default()
        },
    )
    .await
    .expect("session");
    *service.inner.session.write().await = Some(SessionSlot {
        session: session.clone(),
        config: SessionConfig::default(),
        generation: 1,
    });
    session
}

/// A seeding row whose two files lie in its package folder, `from`; `to` is where it moves.
pub(super) struct Seed {
    pub(super) scratch: Scratch,
    pub(super) database: rd_db::Database,
    /// Read by the restart, which only the crash cases drive.
    #[cfg_attr(not(feature = "failpoints"), allow(dead_code))]
    settings: SharedTorrentSettings,
    pub(super) service: TorrentService,
    session: Arc<librqbit::Session>,
    pub(super) id: DownloadId,
    package_id: PackageId,
    pub(super) from: PathBuf,
    pub(super) to: PathBuf,
}

impl Seed {
    pub(super) async fn new() -> Self {
        let scratch = Scratch::new();
        let downloads = scratch.0.join("downloads");
        let from = downloads.join("Release");
        let to = scratch.0.join("archive").join("Release");
        std::fs::create_dir_all(from.join("sub")).expect("package folder");
        std::fs::write(from.join("a.bin"), b"a").expect("payload");
        std::fs::write(from.join("sub").join("b.bin"), b"b").expect("payload");
        let database = rd_db::Database::open(scratch.0.join("torrent.sqlite3"))
            .await
            .expect("database");
        let package = database
            .create_package(rd_db::NewPackage {
                id: PackageId::new(),
                name: "Release".to_owned(),
                destination: from.display().to_string(),
                category_id: None,
                priority: rd_core::DownloadPriority::default(),
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let settings = crate::shared_settings(&database).await.expect("settings");
        let service = TorrentService::start(
            database.clone(),
            settings.clone(),
            scratch.0.clone(),
            downloads.clone(),
        );
        let session = go_offline(&service, downloads).await;
        let (parsed, stored) = service.store_torrent_file(&torrent()).await.expect("store");
        let id = DownloadId::new();
        database
            .create_download(rd_db::NewDownload {
                id,
                package_id: package.id,
                source: url::Url::from_file_path(&stored).expect("file URL"),
                file_name: "Release".to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
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
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Seeding,
        ] {
            database
                .transition_download(id, state)
                .await
                .expect("transition");
        }
        let mut state = service.job_state(id).await;
        state.metadata = Some(parsed.metadata.clone());
        service.store_job_state(id, state).await;
        // Seeding, in the registry the way `begin` leaves it.
        service.inner.registry.write().await.register(
            id,
            usize::MAX,
            parsed.info_hash,
            TorrentPhase::Seeding,
            1,
        );
        Self {
            scratch,
            database,
            settings,
            service,
            session,
            id,
            package_id: package.id,
            from,
            to,
        }
    }

    /// Drops this service the way a stop does and starts a second one over the same data,
    /// whose `recover` runs as every start's does.
    #[cfg(feature = "failpoints")]
    pub(super) async fn restart(&mut self) {
        self.session.cancellation_token().cancel();
        self.service.shutdown();
        let downloads = self.scratch.0.join("downloads");
        self.service = TorrentService::start(
            self.database.clone(),
            self.settings.clone(),
            self.scratch.0.clone(),
            downloads.clone(),
        );
        self.session = go_offline(&self.service, downloads).await;
        self.service.recover().await.expect("recover");
    }

    pub(super) async fn destination(&self) -> PathBuf {
        PathBuf::from(
            self.database
                .get_package(self.package_id)
                .await
                .expect("read")
                .expect("package")
                .destination,
        )
    }

    pub(super) async fn phase(&self) -> Option<TorrentPhase> {
        self.service
            .inner
            .registry
            .read()
            .await
            .get(self.id)
            .map(|entry| entry.phase)
    }

    /// Both files under `folder`, with their bytes.
    pub(super) fn holds_both(folder: &Path) -> bool {
        std::fs::read(folder.join("a.bin")).ok().as_deref() == Some(b"a".as_slice())
            && std::fs::read(folder.join("sub").join("b.bin"))
                .ok()
                .as_deref()
                == Some(b"b".as_slice())
    }
}

impl Drop for Seed {
    fn drop(&mut self) {
        self.session.cancellation_token().cancel();
        self.service.shutdown();
    }
}

#[test]
fn only_plain_names_make_a_path_inside_the_package_folder() {
    let path = |parts: &[&str]| {
        relative_path(
            &parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(path(&["a.bin"]), Some(PathBuf::from("a.bin")));
    assert_eq!(
        path(&["sub", "b.bin"]),
        Some(Path::new("sub").join("b.bin"))
    );
    for unsafe_path in [
        &[][..],
        &[""][..],
        &[".."][..],
        &["sub", "..", "b.bin"][..],
        &["."][..],
        &["sub/b.bin"][..],
        &["/etc"][..],
    ] {
        assert_eq!(path(unsafe_path), None, "{unsafe_path:?}");
    }
}

#[tokio::test]
async fn only_the_torrents_own_empty_folders_are_removed() {
    let scratch = Scratch::new();
    let root = scratch.0.join("Release");
    std::fs::create_dir_all(root.join("x").join("y")).expect("torrent folder");
    std::fs::create_dir_all(root.join("x").join("kept")).expect("someone else's folder");

    remove_empty_folders(&root, &[Path::new("x").join("y").join("file.bin")]).await;

    assert!(!root.join("x").join("y").exists());
    assert!(
        root.join("x").join("kept").exists(),
        "a folder the torrent never had stays"
    );
    assert!(root.exists(), "a folder that is not empty stays");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seed_moves_with_its_files_and_seeds_from_the_new_folder() {
    let seed = Seed::new().await;

    seed.service
        .relocate(seed.id, seed.to.clone())
        .await
        .expect("move");

    assert!(Seed::holds_both(&seed.to), "the files did not arrive");
    assert!(!seed.from.exists(), "the old folder is not empty");
    assert_eq!(seed.destination().await, seed.to);
    let state = seed.service.job_state(seed.id).await;
    assert!(state.relocation.is_none(), "the journal was not cleared");
    assert!(state.relocation_error.is_none());
    assert_eq!(seed.phase().await, Some(TorrentPhase::Seeding));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_that_fails_leaves_everything_where_it_was_and_says_why() {
    let seed = Seed::new().await;
    // The new folder's parent is a file, so not even the first file can be placed.
    let blocker = seed.scratch.0.join("blocker");
    std::fs::write(&blocker, b"in the way").expect("blocker");

    seed.service
        .relocate(seed.id, blocker.join("Release"))
        .await
        .expect_err("a folder below a file cannot be created");

    assert!(
        Seed::holds_both(&seed.from),
        "the files left the old folder"
    );
    assert_eq!(seed.destination().await, seed.from);
    let state = seed.service.job_state(seed.id).await;
    assert!(state.relocation.is_none(), "the journal was not cleared");
    assert!(state.relocation_error.is_some(), "the reason was not kept");
    assert_eq!(seed.phase().await, Some(TorrentPhase::Seeding));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_folder_that_holds_something_is_refused_before_anything_moves() {
    let seed = Seed::new().await;
    std::fs::create_dir_all(&seed.to).expect("target");
    std::fs::write(seed.to.join("notes.txt"), b"mine").expect("foreign file");

    seed.service
        .relocate(seed.id, seed.to.clone())
        .await
        .expect_err("a folder with files in it is refused");

    assert!(Seed::holds_both(&seed.from));
    assert_eq!(
        std::fs::read(seed.to.join("notes.txt")).expect("kept"),
        b"mine"
    );
    assert!(seed.service.job_state(seed.id).await.relocation.is_none());
}

/// "Recheck" finds damaged data: the torrent's one piece does not match what is on disk, so
/// the check reports it short and the live seed fetches it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recheck_hashes_a_seed_again_and_reports_what_did_not_verify() {
    let seed = Seed::new().await;

    assert!(
        seed.service.recheck(seed.id).await.expect("recheck"),
        "a seed is checked in place"
    );
    assert_eq!(seed.phase().await, Some(TorrentPhase::Seeding));

    let mut finished = None;
    for _ in 0..100 {
        let recheck = seed.service.job_state(seed.id).await.recheck;
        if let Some(recheck) = recheck.filter(|recheck| !recheck.is_pending()) {
            finished = Some(recheck);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let recheck = finished.expect("the check never reported");
    assert!(
        recheck.verified_bytes < recheck.total_bytes,
        "damaged data verified: {recheck:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recheck_of_a_torrent_that_is_not_seeding_runs_when_it_starts_next() {
    let seed = Seed::new().await;
    seed.database
        .transition_download(seed.id, DownloadState::Completed)
        .await
        .expect("leave seeding");

    assert!(
        !seed.service.recheck(seed.id).await.expect("recheck"),
        "only a seed is checked in place"
    );
    let recheck = seed.service.job_state(seed.id).await.recheck;
    assert!(recheck.is_some_and(|recheck| recheck.is_pending()));
    assert_eq!(seed.phase().await, None, "the torrent left the session");
}

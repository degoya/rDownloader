//! Crash and restart of a seed (RD-180-12, recovery matrix).
//!
//! What is exercised is the queue row and the seed clock, not the swarm: both sessions are
//! offline, the same options `forget_tests` restores with, and the torrent's one piece matches
//! nothing. The restart is a second service over the same database whose `recover` runs, which
//! is what every start does.

use std::{path::PathBuf, sync::Arc};

use rd_core::{DownloadId, DownloadKind, DownloadState, PackageId, failpoint::FailpointGuard};

use crate::{
    TorrentService,
    registry::TorrentPhase,
    session::{SessionConfig, SessionSlot},
};

/// A directory below the system temp dir, removed again when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rd-torrent-seed-{}", DownloadId::new()));
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

/// A one-byte single-file torrent.
fn torrent() -> Vec<u8> {
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("announce"));
    bytes.extend(bstr("http://tracker.example/announce"));
    bytes.extend(bstr("info"));
    bytes.push(b'd');
    bytes.extend(bstr("length"));
    bytes.extend_from_slice(b"i1e");
    bytes.extend(bstr("name"));
    bytes.extend(bstr("seed.bin"));
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

/// `torrent.before_seed_completed`: the seed time is closed, the row still says seeding.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seed_stopped_before_its_row_completed_is_taken_up_again_and_counts_its_time_once() {
    let scratch = Scratch::new();
    let downloads = scratch.0.join("downloads");
    let database = rd_db::Database::open(scratch.0.join("torrent.sqlite3"))
        .await
        .expect("database");
    let package = database
        .create_package(rd_db::NewPackage {
            id: PackageId::new(),
            name: "seeded".to_owned(),
            destination: downloads.join("seeded").display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let settings = crate::shared_settings(&database).await.expect("settings");
    let first = TorrentService::start(
        database.clone(),
        settings.clone(),
        scratch.0.clone(),
        downloads.clone(),
    );
    let first_session = go_offline(&first, downloads.clone()).await;
    let (parsed, stored) = first.store_torrent_file(&torrent()).await.expect("store");
    let id = DownloadId::new();
    database
        .create_download(rd_db::NewDownload {
            id,
            package_id: package.id,
            source: url::Url::from_file_path(&stored).expect("file URL"),
            file_name: "seed.bin".to_owned(),
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
    // Seeding for a minute and a half, in the registry the way `begin` leaves it.
    let mut state = first.job_state(id).await;
    state
        .seed
        .start(chrono::Utc::now() - chrono::Duration::seconds(90));
    first.store_job_state(id, state).await;
    first.inner.registry.write().await.register(
        id,
        usize::MAX,
        parsed.info_hash.clone(),
        TorrentPhase::Seeding,
        1,
    );

    {
        let guard = FailpointGuard::once("torrent.before_seed_completed");
        assert!(
            crate::seeding::stop(&first, id, "ratio reached")
                .await
                .is_err()
        );
        assert!(guard.fired(), "the crash point was never reached");
    }
    first_session.cancellation_token().cancel();
    first.shutdown();
    let row = database.get_download(id).await.expect("row").expect("row");
    assert_eq!(row.state, DownloadState::Seeding);
    let closed = first.job_state(id).await.seed;
    assert!(closed.started_at.is_none(), "{closed:?}");
    assert!(closed.accumulated_seconds >= 90, "{closed:?}");

    let restarted = TorrentService::start(
        database.clone(),
        settings,
        scratch.0.clone(),
        downloads.clone(),
    );
    let session = go_offline(&restarted, downloads).await;
    restarted.recover().await.expect("recover");
    let phase = restarted
        .inner
        .registry
        .read()
        .await
        .get(id)
        .map(|entry| entry.phase);
    assert_eq!(
        phase,
        Some(TorrentPhase::Seeding),
        "the restart did not take the seed up again"
    );
    // The time the first run closed, not that twice.
    assert_eq!(restarted.job_state(id).await.seed, closed);

    assert!(restarted.stop_seeding(id).await.expect("stop"));
    let row = database.get_download(id).await.expect("row").expect("row");
    assert_eq!(row.state, DownloadState::Completed);
    assert_eq!(
        restarted.job_state(id).await.seed.accumulated_seconds,
        closed.accumulated_seconds
    );
    session.cancellation_token().cancel();
    restarted.shutdown();
}

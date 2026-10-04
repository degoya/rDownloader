//! A torrent's reviewed selection is on its row before the row can start (audit 1.9.1, API-07).
//!
//! The LinkGrabber used to write the selection after the whole package was enqueued; a
//! dispatcher pass in between started the torrent with the default selection. The scheduler
//! here admits no transfer at all (`max_active_files: 0`), so every state below is the
//! enqueue's doing, not a race with a worker.

use std::{collections::BTreeMap, path::Path};

use rd_core::{DownloadState, TorrentJobState};
use rd_scheduler::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

async fn installation(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("torrent-selection.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: 0,
            ..SchedulerConfig::for_directory(directory.join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    (scheduler, database)
}

fn package(directory: &Path, start_paused: bool) -> PackageSpec {
    PackageSpec {
        name: format!("torrent {}", rd_core::DownloadId::new()),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    }
}

fn file(source: &str, kind: rd_core::DownloadKind) -> FileSpec {
    FileSpec {
        source: source.parse().expect("url"),
        file_name: "release".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        kind,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }
}

/// A selection that is not the default one: the second file left out.
fn reviewed() -> TorrentJobState {
    let mut state = TorrentJobState::default();
    state.plan.explicit = BTreeMap::from([(0, true), (1, false)]);
    state
}

const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn the_reviewed_selection_is_stored_and_the_row_then_joins_the_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let (_, files) = scheduler
        .enqueue_package_with_torrents(
            package(directory.path(), false),
            vec![
                file(MAGNET, rd_core::DownloadKind::Torrent),
                file(
                    "https://example.invalid/notes.txt",
                    rd_core::DownloadKind::Http,
                ),
            ],
            vec![(MAGNET.parse().expect("url"), reviewed())],
        )
        .await
        .expect("enqueue");

    let torrent = files
        .iter()
        .find(|file| file.source.as_str() == MAGNET)
        .expect("torrent row");
    assert_eq!(torrent.state, DownloadState::Queued);
    let stored = database
        .download_torrent_state(torrent.id)
        .await
        .expect("read")
        .expect("a stored selection");
    assert_eq!(stored.plan.explicit, reviewed().plan.explicit);

    // The file without a state is untouched by any of this.
    let plain = files
        .iter()
        .find(|file| file.source.as_str() != MAGNET)
        .expect("plain row");
    assert_eq!(plain.state, DownloadState::Queued);
    assert!(
        database
            .download_torrent_state(plain.id)
            .await
            .expect("read")
            .is_none()
    );
}

#[tokio::test]
async fn a_package_asked_to_start_paused_stays_paused_with_its_selection() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let (_, files) = scheduler
        .enqueue_package_with_torrents(
            package(directory.path(), true),
            vec![file(MAGNET, rd_core::DownloadKind::Torrent)],
            vec![(MAGNET.parse().expect("url"), reviewed())],
        )
        .await
        .expect("enqueue");

    let torrent = files.first().expect("torrent row");
    assert_eq!(torrent.state, DownloadState::Paused);
    let stored = database
        .download_torrent_state(torrent.id)
        .await
        .expect("read")
        .expect("a stored selection");
    assert_eq!(stored.plan.explicit, reviewed().plan.explicit);
}

/// RA-API-06: the row joins the queue without a `paused` → `queued` transition of its own. A
/// row created queued announces none, so automations, notifications and the event stream saw
/// a state change for torrents that no other download has.
#[tokio::test]
async fn joining_the_queue_announces_no_transition() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let mut events = database.subscribe();
    let (_, files) = scheduler
        .enqueue_package_with_torrents(
            package(directory.path(), false),
            vec![file(MAGNET, rd_core::DownloadKind::Torrent)],
            vec![(MAGNET.parse().expect("url"), reviewed())],
        )
        .await
        .expect("enqueue");
    let torrent = files.first().expect("torrent row");
    assert_eq!(torrent.state, DownloadState::Queued);

    let id = torrent.id.to_string();
    while let Ok(event) = events.try_recv() {
        if event.kind != rd_core::EventKind::DownloadState
            || event.payload["download_id"].as_str() != Some(id.as_str())
        {
            continue;
        }
        assert!(
            event.payload.get("state").is_none() && event.payload.get("previous").is_none(),
            "the enqueue announced a transition: {}",
            event.payload
        );
    }
}

/// RA-TR-08: the queue step reads the row again instead of writing over it from the enqueue's
/// snapshot. A row somebody resumed and paused again since it was written stays paused.
#[tokio::test]
async fn joining_the_queue_leaves_a_row_touched_in_between_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let (_, files) = scheduler
        .enqueue_package_with_torrents(
            package(directory.path(), true),
            vec![file(MAGNET, rd_core::DownloadKind::Torrent)],
            vec![(MAGNET.parse().expect("url"), reviewed())],
        )
        .await
        .expect("enqueue");
    let written = files.first().expect("torrent row").clone();
    assert_eq!(written.state, DownloadState::Paused);

    // The person's resume and pause, between the write and the queue step.
    database
        .transition_download(written.id, DownloadState::Queued)
        .await
        .expect("resume");
    database
        .transition_download(written.id, DownloadState::Paused)
        .await
        .expect("pause");

    let after = database
        .join_queue(written.id, written.updated_at)
        .await
        .expect("join");
    assert_eq!(
        after.state,
        DownloadState::Paused,
        "the pause that came in between was overwritten"
    );

    // Untouched, the same step does queue it.
    let untouched = database
        .get_download(written.id)
        .await
        .expect("read")
        .expect("row");
    let joined = database
        .join_queue(untouched.id, untouched.updated_at)
        .await
        .expect("join");
    assert_eq!(joined.state, DownloadState::Queued);
}

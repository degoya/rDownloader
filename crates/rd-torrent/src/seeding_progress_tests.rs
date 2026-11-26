//! The seeding sweep announces nothing while nothing a seed shows has changed (RD-1480-18).

use rd_core::{AuthProfileSelection, DownloadId, DownloadKind, DownloadState, PackageId};

use super::show_progress;

#[tokio::test]
async fn a_sweep_over_an_unchanged_seed_announces_nothing() {
    let scratch = std::env::temp_dir().join(format!("rd-torrent-sweep-{}", DownloadId::new()));
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let database = rd_db::Database::open(scratch.join("torrent.sqlite3"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "seed".to_owned(),
            destination: scratch.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
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
            package_id,
            source: "magnet:?xt=urn:btih:0000000000000000000000000000000000000000"
                .parse()
                .expect("URL"),
            file_name: "seed".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
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
    show_progress(&database, id, 1_000, 1_000).await;
    let mut events = database.subscribe();

    for _ in 0..3 {
        show_progress(&database, id, 1_000, 1_000).await;
    }
    assert!(events.try_recv().is_err(), "the row shows these bytes");

    // A recheck found damaged pieces: what is shown changes, and the sweep writes it.
    show_progress(&database, id, 400, 1_000).await;
    let row = database.get_download(id).await.expect("read").expect("row");
    assert_eq!(row.committed_bytes.get(), 400);
    show_progress(&database, DownloadId::new(), 1, 1).await;
    let _ = std::fs::remove_dir_all(&scratch);
}

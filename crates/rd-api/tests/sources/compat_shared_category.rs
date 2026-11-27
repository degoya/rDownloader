//! The SABnzbd and the qBittorrent adapter side by side, under one category (RD-1490-13).
//!
//! Sonarr and Radarr each poll both clients and keep what carries the category they set. With
//! the same category on both, a torrent the SABnzbd adapter also listed was imported twice by
//! Sonarr and credited to the SABnzbd client by Radarr. Each adapter reports its own kind only:
//! SABnzbd the Usenet packages, qBittorrent the torrents.

use crate::common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::API_BEARER;

use crate::compat_arr::{minimal_nzb, seed_category};

const BOUNDARY: &str = "----shared";
const HASH: &str = "fedcba9876543210fedcba9876543210fedcba98";
const CATEGORY: &str = "tv";

/// Parked: the states are written by hand, and a live scheduler would race them for the rows.
async fn harness(directory: &std::path::Path) -> common::Harness {
    let harness = common::harness(directory, common::Options::default().login().parked()).await;
    seed_category(&harness, directory, CATEGORY).await;
    harness
}

fn request(method: &str, uri: &str, content_type: Option<&str>, body: Vec<u8>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        // The qBittorrent session; the SABnzbd side reads its key from the query.
        .header(header::COOKIE, format!("SID={API_BEARER}"));
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(body)).expect("request")
}

async fn call(harness: &common::Harness, request: Request<Body>) -> serde_json::Value {
    let (status, _, body) = common::send_text(&harness.router, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str(&body).unwrap_or_else(|error| panic!("not JSON ({error}): {body}"))
}

/// A SABnzbd `GET` of `query` (`mode=…`).
async fn sab(harness: &common::Harness, query: &str) -> serde_json::Value {
    let uri = format!("/api?{query}&apikey={API_BEARER}&output=json");
    call(harness, request("GET", &uri, None, Vec::new())).await
}

/// `mode=addfile` the way Sonarr sends it, `cat` and `priority` in the query; the job id.
async fn add_nzb(harness: &common::Harness, release: &str, priority: &str) -> String {
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"name\"; filename=\"{release}.nzb\"\r\nContent-Type: application/x-nzb\r\n\r\n{}\r\n--{BOUNDARY}--\r\n",
        minimal_nzb(release)
    );
    let uri = format!(
        "/api?mode=addfile&cat={CATEGORY}&priority={priority}&apikey={API_BEARER}&output=json"
    );
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    let added = call(
        harness,
        request("POST", &uri, Some(&content_type), body.into_bytes()),
    )
    .await;
    assert_eq!(added["status"], true, "{added}");
    added["nzo_ids"][0].as_str().expect("nzo id").to_owned()
}

/// `torrents/add` with a magnet, the way Sonarr sends it; the torrent's package.
async fn add_torrent(harness: &common::Harness) -> rd_core::PackageId {
    let form = format!(
        "urls=magnet%3A%3Fxt%3Durn%3Abtih%3A{HASH}%26dn%3DShared.Show.S01E02&category={CATEGORY}&paused=false"
    );
    let (status, _, body) = common::send_text(
        &harness.router,
        request(
            "POST",
            "/api/v2/torrents/add",
            Some("application/x-www-form-urlencoded"),
            form.into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let packages = harness.database.list_packages().await.expect("packages");
    let torrent = packages
        .iter()
        .find(|package| package.name == "Shared.Show.S01E02")
        .unwrap_or_else(|| panic!("the torrent was not queued: {packages:?}"));
    assert_eq!(torrent.kind, rd_core::DownloadKind::Torrent);
    torrent.id
}

fn slot_ids(listing: &serde_json::Value, list: &str) -> Vec<String> {
    listing[list]["slots"]
        .as_array()
        .unwrap_or_else(|| panic!("{list} without slots: {listing}"))
        .iter()
        .map(|slot| slot["nzo_id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

async fn qbittorrent_hashes(harness: &common::Harness) -> Vec<String> {
    let uri = format!("/api/v2/torrents/info?category={CATEGORY}");
    call(harness, request("GET", &uri, None, Vec::new()))
        .await
        .as_array()
        .expect("torrent list")
        .iter()
        .map(|entry| entry["hash"].as_str().unwrap_or_default().to_owned())
        .collect()
}

async fn finish(harness: &common::Harness, package: rd_core::PackageId) {
    harness
        .database
        .set_package_state(package, rd_core::PackageState::Completed, None, None, None)
        .await
        .expect("completed");
}

/// Removes a package's rows, so it lives on in the download history only.
async fn remove(harness: &common::Harness, package: rd_core::PackageId) {
    for file in harness
        .database
        .downloads_for_package(package)
        .await
        .expect("files")
    {
        harness
            .database
            .delete_download(file.id)
            .await
            .expect("removed");
    }
}

#[tokio::test]
async fn under_one_category_each_adapter_lists_its_own_kind_in_queue_and_history() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let nzo_id = add_nzb(&harness, "Shared.Show.S01E01", "-100").await;
    let torrent = add_torrent(&harness).await;

    let queue = sab(&harness, "mode=queue").await;
    assert_eq!(
        slot_ids(&queue, "queue"),
        std::slice::from_ref(&nzo_id),
        "{queue}"
    );
    assert_eq!(queue["queue"]["noofslots"], 1, "{queue}");
    assert_eq!(qbittorrent_hashes(&harness).await, [HASH]);

    // Finished, both are history; then gone from the queue, both live on in the download
    // history, and SABnzbd's still carries the NZB alone.
    let nzb: rd_core::PackageId = nzo_id
        .strip_prefix("rd_nzo_")
        .and_then(|id| id.parse().ok())
        .expect("our job id");
    for package in [nzb, torrent] {
        finish(&harness, package).await;
    }
    let history = sab(&harness, "mode=history").await;
    assert_eq!(
        slot_ids(&history, "history"),
        std::slice::from_ref(&nzo_id),
        "{history}"
    );
    for package in [nzb, torrent] {
        remove(&harness, package).await;
    }
    let history = sab(&harness, "mode=history").await;
    assert_eq!(
        slot_ids(&history, "history"),
        std::slice::from_ref(&nzo_id),
        "{history}"
    );

    // The native history keeps both, each with its own kind: a torrent was `http` before.
    let native = harness
        .database
        .list_download_history(&rd_db::HistoryQuery::default())
        .await
        .expect("native history");
    let kind_of = |package: rd_core::PackageId| {
        native
            .entries
            .iter()
            .find(|entry| entry.package_id == package)
            .map(|entry| entry.kind)
    };
    assert_eq!(kind_of(nzb), Some(rd_core::DownloadKind::Usenet));
    assert_eq!(kind_of(torrent), Some(rd_core::DownloadKind::Torrent));
}

#[tokio::test]
async fn a_torrent_s_id_is_no_sabnzbd_job_and_pausing_everything_leaves_it_running() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let nzo_id = add_nzb(&harness, "Shared.Show.S01E01", "-100").await;
    let torrent = add_torrent(&harness).await;

    for name in ["delete", "pause", "resume"] {
        let answer = sab(
            &harness,
            &format!("mode=queue&name={name}&value=rd_nzo_{torrent}"),
        )
        .await;
        assert_eq!(answer["status"], false, "{name}: {answer}");
        assert_eq!(answer["error"], "nzo_id not found", "{name}: {answer}");
    }
    let answer = sab(
        &harness,
        &format!("mode=history&name=delete&value=rd_nzo_{torrent}"),
    )
    .await;
    assert_eq!(answer["status"], true, "{answer}");
    assert_eq!(qbittorrent_hashes(&harness).await, [HASH]);

    // SABnzbd's `pause` stops its own jobs, not the other client's torrent.
    let answer = sab(&harness, "mode=pause").await;
    assert_eq!(answer["status"], true, "{answer}");
    for file in harness.database.list_downloads().await.expect("downloads") {
        // The torrent engine may already have picked the magnet up; paused it is not.
        let paused = file.state == rd_core::DownloadState::Paused;
        assert_eq!(paused, file.package_id != torrent, "{}", file.file_name);
    }
    let queue = sab(&harness, "mode=queue").await;
    assert_eq!(queue["queue"]["slots"][0]["nzo_id"], nzo_id, "{queue}");
    assert_eq!(queue["queue"]["slots"][0]["status"], "Paused", "{queue}");

    // `value=all` clears this client's queue and leaves the torrent alone.
    let answer = sab(&harness, "mode=queue&name=delete&value=all").await;
    assert_eq!(answer["status"], true, "{answer}");
    assert!(slot_ids(&sab(&harness, "mode=queue").await, "queue").is_empty());
    assert_eq!(qbittorrent_hashes(&harness).await, [HASH]);
}

/// SABnzbd's `priority=-2` is "paused": the job is queued and waits instead of starting.
#[tokio::test]
async fn addfile_with_priority_minus_two_is_queued_paused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let paused = add_nzb(&harness, "Paused.Release", "-2").await;
    let high = add_nzb(&harness, "High.Release", "1").await;

    let package = |nzo_id: &str| -> rd_core::PackageId {
        nzo_id
            .strip_prefix("rd_nzo_")
            .and_then(|id| id.parse().ok())
            .expect("our job id")
    };
    let files = harness
        .database
        .downloads_for_package(package(&paused))
        .await
        .expect("files");
    assert!(!files.is_empty());
    assert!(
        files
            .iter()
            .all(|file| file.state == rd_core::DownloadState::Paused),
        "{files:?}"
    );
    let queue = sab(&harness, "mode=queue").await;
    let slot = |nzo_id: &str| {
        queue["queue"]["slots"]
            .as_array()
            .and_then(|slots| slots.iter().find(|slot| slot["nzo_id"] == nzo_id))
            .cloned()
            .unwrap_or_else(|| panic!("{nzo_id} is not in the queue: {queue}"))
    };
    assert_eq!(slot(&paused)["status"], "Paused", "{queue}");
    assert_eq!(slot(&high)["status"], "Queued", "{queue}");
    assert_eq!(slot(&high)["priority"], "High", "{queue}");
    assert_eq!(
        harness
            .database
            .get_package(package(&high))
            .await
            .expect("read")
            .map(|package| package.priority),
        Some(rd_core::DownloadPriority::High)
    );
}

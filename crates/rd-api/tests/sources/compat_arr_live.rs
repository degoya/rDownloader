//! The three requests Sonarr 4 and Radarr 6 failed on in the live run of RD-1400-06
//! (`docs/compatibility.md`, *Live run*), sent the way those applications send them
//! (RD-1430-12): the NZB in the multipart part `name`, a magnet as a url-encoded form, and the
//! `save_path` the import compares with `content_path`.

use crate::common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{API_BEARER, auth_harness};

use crate::compat_arr::{minimal_nzb, seed_category};

const BOUNDARY: &str = "----arrlive";
const HASH: &str = "89abcdef0123456789abcdef0123456789abcdef";

fn sab_key() -> String {
    format!("apikey={API_BEARER}&output=json")
}

/// A `POST` with the qBittorrent session cookie, or none for the SABnzbd side.
fn post(uri: &str, content_type: &str, body: Vec<u8>, cookie: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, content_type);
    if cookie {
        builder = builder.header(header::COOKIE, format!("SID={API_BEARER}"));
    }
    builder.body(Body::from(body)).expect("request")
}

/// A `GET` with the qBittorrent session cookie.
fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::COOKIE, format!("SID={API_BEARER}"))
        .body(Body::empty())
        .expect("request")
}

/// A SABnzbd `GET`, the key in the query.
fn sab_get(mode: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api?mode={mode}&{}", sab_key()))
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request")
}

async fn send(router: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let (status, _, body) = common::send_text(router, request).await;
    (status, body)
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|error| panic!("not JSON ({error}): {body}"))
}

fn multipart_type() -> String {
    format!("multipart/form-data; boundary={BOUNDARY}")
}

/// One multipart file part, as `AddFormUpload` writes it.
fn file_part(name: &str, file_name: &str, content: &str) -> String {
    format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: application/x-nzb\r\n\r\n{content}\r\n"
    )
}

/// `mode=addfile` as Sonarr and Radarr call it: `cat` and `priority` in the query.
async fn addfile(router: &axum::Router, parts: &str) -> serde_json::Value {
    let body = format!("{parts}--{BOUNDARY}--\r\n");
    let (status, body) = send(
        router,
        post(
            &format!("/api?mode=addfile&cat=tv&priority=-100&{}", sab_key()),
            &multipart_type(),
            body.into_bytes(),
            false,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    json(&body)
}

async fn sab_queue_names(router: &axum::Router) -> Vec<String> {
    let (_, body) = send(router, sab_get("queue")).await;
    json(&body)["queue"]["slots"]
        .as_array()
        .unwrap_or_else(|| panic!("queue without slots: {body}"))
        .iter()
        .map(|slot| slot["filename"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Sonarr's and Radarr's grab: the NZB in `name="name"; filename="<release>.nzb"`.
#[tokio::test]
async fn sab_addfile_takes_the_nzb_from_the_part_called_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    seed_category(&harness, directory.path(), "tv").await;

    let release = "Some.Show.S01E01";
    let added = addfile(
        &harness.router,
        &file_part("name", &format!("{release}.nzb"), &minimal_nzb(release)),
    )
    .await;
    assert_eq!(added["status"], true, "{added}");
    let nzo_id = added["nzo_ids"][0].as_str().expect("nzo id").to_owned();

    let (_, body) = send(&harness.router, sab_get("queue")).await;
    let queue = json(&body);
    let slot = queue["queue"]["slots"]
        .as_array()
        .and_then(|slots| slots.iter().find(|slot| slot["nzo_id"] == nzo_id))
        .unwrap_or_else(|| panic!("the grab is missing from the queue: {body}"));
    assert_eq!(slot["cat"], "tv", "{body}");
    assert!(
        slot["filename"]
            .as_str()
            .is_some_and(|name| name.contains(release)),
        "the job is named after the uploaded file: {body}"
    );
}

/// `nzbfile` before `file` before `name`, whichever comes first in the body.
#[tokio::test]
async fn sab_addfile_prefers_nzbfile_over_name_in_either_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    // Two different NZBs, so neither add could be taken for a repeat of the other.
    let preferred =
        |release: &str| file_part("nzbfile", &format!("{release}.nzb"), &minimal_nzb(release));
    let other = file_part("name", "Passed.Over.nzb", &minimal_nzb("Passed.Over"));

    for parts in [
        format!("{other}{}", preferred("Preferred.A")),
        format!("{}{other}", preferred("Preferred.B")),
    ] {
        let added = addfile(&harness.router, &parts).await;
        assert_eq!(added["status"], true, "{added}");
    }
    let names = sab_queue_names(&harness.router).await;
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names
            .iter()
            .all(|name| name.contains("Preferred") && !name.contains("Passed")),
        "{names:?}"
    );
}

/// A `name` part without a file is a value, not an NZB; without a file part it stays a refusal.
#[tokio::test]
async fn sab_addfile_without_a_file_part_is_still_no_nzb_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let text_only = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nSome.Show.S01E01\r\n"
    );
    let refused = addfile(&harness.router, &text_only).await;
    assert_eq!(refused["status"], false, "{refused}");
    assert_eq!(refused["error"], "no nzb file", "{refused}");
    assert!(sab_queue_names(&harness.router).await.is_empty());
}

/// Sonarr's and Radarr's magnet add: `urls`, `category`, `paused` as a url-encoded form.
#[tokio::test]
async fn qbittorrent_add_takes_a_magnet_as_a_url_encoded_form() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    seed_category(&harness, directory.path(), "tv-sonarr").await;

    let form = format!(
        "urls=magnet%3A%3Fxt%3Durn%3Abtih%3A{HASH}%26dn%3DSome.Show.S01E02&category=tv-sonarr&paused=false"
    );
    let (status, body) = send(
        &harness.router,
        post(
            "/api/v2/torrents/add",
            "application/x-www-form-urlencoded",
            form.into_bytes(),
            true,
        ),
    )
    .await;
    // An empty 200, as older qBittorrent answers; Sonarr and Radarr fail only on `Fails.`.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(body, "Fails.");

    let entry = info_entry(&harness.router).await;
    assert_eq!(entry["hash"], HASH, "{entry}");
    assert_eq!(entry["category"], "tv-sonarr", "{entry}");
    assert_eq!(entry["name"], "Some.Show.S01E02", "{entry}");
}

/// The form path refuses what the multipart path refuses, and every other body is a `415`.
#[tokio::test]
async fn qbittorrent_add_refuses_what_multipart_refuses_and_other_content_types() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let magnet = format!("urls=magnet%3A%3Fxt%3Durn%3Abtih%3A{HASH}");

    for (content_type, body) in [
        // Not a magnet: the service would fetch a URL the caller chose.
        (
            "application/x-www-form-urlencoded",
            "urls=https%3A%2F%2Fexample.com%2Fa.torrent".to_owned(),
        ),
        // Nothing to add.
        (
            "application/x-www-form-urlencoded",
            "category=tv-sonarr&paused=false".to_owned(),
        ),
        // Neither multipart nor a form.
        ("text/plain", magnet.clone()),
        ("application/json", format!("{{\"urls\":\"{magnet}\"}}")),
    ] {
        let (status, answer) = send(
            &harness.router,
            post(
                "/api/v2/torrents/add",
                content_type,
                body.clone().into_bytes(),
                true,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{content_type} {body}: {answer}"
        );
    }
    let (_, body) = send(&harness.router, get("/api/v2/torrents/info")).await;
    assert_eq!(json(&body).as_array().map(Vec::len), Some(0), "{body}");
}

/// A magnet before its metadata is a folder: `content_path` the package folder, `save_path`
/// the folder it lies in -- never the same path, which the import refuses.
#[tokio::test]
async fn qbittorrent_reports_the_package_folder_as_content_and_its_parent_as_save_path() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let form = format!(
        "urls=magnet%3A%3Fxt%3Durn%3Abtih%3A{HASH}%26dn%3DSome.Movie.2024&category=&paused=false"
    );
    let (status, _) = send(
        &harness.router,
        post(
            "/api/v2/torrents/add",
            "application/x-www-form-urlencoded",
            form.into_bytes(),
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let entry = info_entry(&harness.router).await;
    let (content, save) = content_and_save(&entry);
    assert_ne!(content, save, "{entry}");
    assert_eq!(content.parent(), Some(save.as_path()), "{entry}");
    assert_eq!(
        content.file_name().and_then(|name| name.to_str()),
        Some("Some.Movie.2024"),
        "the package folder: {entry}"
    );
    assert_eq!(
        entry["download_path"], "",
        "no separate incomplete folder: {entry}"
    );

    let (status, body) = send(
        &harness.router,
        get(&format!("/api/v2/torrents/properties?hash={HASH}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["save_path"], entry["save_path"], "{body}");
}

/// A single-file torrent's content is its file, inside the package folder `save_path` names.
#[tokio::test]
async fn qbittorrent_reports_a_single_file_torrent_s_file_as_its_content() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Parked: the torrent is only looked at, never started.
    let harness = common::harness(
        directory.path(),
        common::Options::default().login().parked(),
    )
    .await;
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"torrents\"; filename=\"release.torrent\"\r\nContent-Type: application/x-bittorrent\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&single_file_torrent());
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    let (status, answer) = send(
        &harness.router,
        post("/api/v2/torrents/add", &multipart_type(), body, true),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let entry = info_entry(&harness.router).await;
    let (content, save) = content_and_save(&entry);
    assert_eq!(
        content.file_name().and_then(|name| name.to_str()),
        Some("release"),
        "the file itself: {entry}"
    );
    assert_eq!(content.parent(), Some(save.as_path()), "{entry}");
    assert_ne!(content, save, "{entry}");
}

/// The one torrent `torrents/info` lists.
async fn info_entry(router: &axum::Router) -> serde_json::Value {
    let (status, body) = send(router, get("/api/v2/torrents/info")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = json(&body);
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{body}");
    listed[0].clone()
}

fn content_and_save(entry: &serde_json::Value) -> (std::path::PathBuf, std::path::PathBuf) {
    let path = |field: &str| {
        std::path::PathBuf::from(
            entry[field]
                .as_str()
                .unwrap_or_else(|| panic!("{field} is not a string: {entry}")),
        )
    };
    (path("content_path"), path("save_path"))
}

/// A single-file torrent named `release`, the smallest one the parser takes.
fn single_file_torrent() -> Vec<u8> {
    let mut torrent =
        b"d8:announce31:http://tracker.example/announce4:infod6:lengthi32e4:name7:release12:piece lengthi16384e6:pieces20:"
            .to_vec();
    torrent.extend_from_slice(&[0_u8; 20]);
    torrent.extend_from_slice(b"ee");
    torrent
}

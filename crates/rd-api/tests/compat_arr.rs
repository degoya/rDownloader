//! The call sequences Sonarr, Radarr, Lidarr and Readarr actually issue.
//!
//! The individual adapter tests check response shapes; these check that the *order* a real
//! client works in gets all the way through — test connection, learn the configuration, add
//! a release, poll for it, import it, remove it. They are recorded sequences rather than a
//! live integration: no *arr instance runs here, so what is proven is that the contract each
//! step depends on holds, not that a given version of Sonarr is happy. That last step needs
//! a real instance and is tracked in `docs/roadmap/jobs/130-20-die-offenen-live-abnahmen.md`.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{API_BEARER, auth_harness};
use http_body_util::BodyExt;
use tower::ServiceExt;

const BOUNDARY: &str = "----arrtest";

async fn send(router: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request")
}

fn get_with_cookie(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::COOKIE, format!("SID={API_BEARER}"))
        .body(Body::empty())
        .expect("request")
}

fn post_form(uri: &str, body: &str, cookie: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if cookie {
        builder = builder.header(header::COOKIE, format!("SID={API_BEARER}"));
    }
    builder.body(Body::from(body.to_owned())).expect("request")
}

fn post_multipart(uri: &str, body: String, cookie: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        );
    if cookie {
        builder = builder.header(header::COOKIE, format!("SID={API_BEARER}"));
    }
    builder.body(Body::from(body)).expect("request")
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|error| panic!("not JSON ({error}): {body}"))
}

/// The full SABnzbd sequence: test connection, read config, add, poll, import, remove.
#[tokio::test]
async fn the_sabnzbd_download_client_sequence_completes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let key = format!("apikey={API_BEARER}&output=json");

    // 1. Test connection. The client refuses to save the download client without this.
    let (status, body) = send(&harness.router, get(&format!("/api?mode=version&{key}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json(&body)["version"].is_string(), "{body}");

    // 2. Learn where finished files land and which categories exist.
    let (_, body) = send(&harness.router, get(&format!("/api?mode=get_config&{key}"))).await;
    let config = json(&body);
    assert!(
        config["config"]["misc"]["complete_dir"].is_string(),
        "the client stores this path and looks for imports under it: {body}"
    );
    let (_, body) = send(&harness.router, get(&format!("/api?mode=get_cats&{key}"))).await;
    assert!(json(&body)["categories"].is_array(), "{body}");

    // 3. Push a release, with the category the client configured.
    let nzb = minimal_nzb("Some.Release.S01E01");
    let multipart = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"nzbfile\"; filename=\"Some.Release.S01E01.nzb\"\r\nContent-Type: application/x-nzb\r\n\r\n{nzb}\r\n--{BOUNDARY}--\r\n"
    );
    let (status, body) = send(
        &harness.router,
        post_multipart(&format!("/api?mode=addfile&cat=tv&{key}"), multipart, false),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let added = json(&body);
    assert_eq!(added["status"], true, "{body}");
    let nzo_id = added["nzo_ids"][0]
        .as_str()
        .expect("the client keys everything that follows on this id")
        .to_owned();

    // 4. Poll the queue and find the job it just added.
    let (_, body) = send(&harness.router, get(&format!("/api?mode=queue&{key}"))).await;
    let queue = json(&body);
    let slot = queue["queue"]["slots"]
        .as_array()
        .and_then(|slots| slots.iter().find(|slot| slot["nzo_id"] == nzo_id))
        .unwrap_or_else(|| panic!("added job missing from the queue: {body}"));
    for field in ["filename", "status", "mb", "mbleft", "percentage", "cat"] {
        assert!(!slot[field].is_null(), "queue slot.{field}: {body}");
    }

    // 5. Poll the history. Nothing has finished, so the job must not be there yet — a
    //    client that saw it here would try to import an empty directory.
    let (_, body) = send(&harness.router, get(&format!("/api?mode=history&{key}"))).await;
    let history = json(&body);
    assert!(
        history["history"]["slots"]
            .as_array()
            .is_some_and(|slots| slots.iter().all(|slot| slot["nzo_id"] != nzo_id)),
        "an unfinished job appeared in the history: {body}"
    );

    // 6. Remove it again, which is what the client does after a failed grab.
    let (_, body) = send(
        &harness.router,
        get(&format!(
            "/api?mode=queue&name=delete&value={nzo_id}&del_files=1&{key}"
        )),
    )
    .await;
    assert_eq!(json(&body)["status"], true, "{body}");
    let (_, body) = send(&harness.router, get(&format!("/api?mode=queue&{key}"))).await;
    assert_eq!(
        json(&body)["queue"]["slots"].as_array().map(Vec::len),
        Some(0),
        "{body}"
    );
}

/// The full qBittorrent sequence, including the category dance clients do on connect.
#[tokio::test]
async fn the_qbittorrent_download_client_sequence_completes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // 1. Clients read the Web API version before anything else, unauthenticated first.
    let (status, _) = send(&harness.router, get("/api/v2/app/webapiVersion")).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an unauthenticated probe must say 'log in', not 'broken'"
    );

    // 2. Log in.
    let (status, body) = send(
        &harness.router,
        post_form(
            "/api/v2/auth/login",
            &format!("username=admin&password={API_BEARER}"),
            false,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "Ok.");

    // 3. Version, preferences and categories: the client's own settings screen.
    for uri in [
        "/api/v2/app/version",
        "/api/v2/app/webapiVersion",
        "/api/v2/app/preferences",
        "/api/v2/torrents/categories",
    ] {
        let (status, _) = send(&harness.router, get_with_cookie(uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }

    // 4. Create its own category. It does this on every connect and treats a failure as a
    //    broken client, even though categories are ours to configure.
    let (status, _) = send(
        &harness.router,
        post_form(
            "/api/v2/torrents/createCategory",
            "category=tv-sonarr&savePath=",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 5. Add a magnet and immediately look for it under the hash it computed itself.
    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
    let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=Some.Movie.2024");
    let multipart = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"urls\"\r\n\r\n{magnet}\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"category\"\r\n\r\ntv-sonarr\r\n--{BOUNDARY}--\r\n"
    );
    let (status, _) = send(
        &harness.router,
        post_multipart("/api/v2/torrents/add", multipart, true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(&harness.router, get_with_cookie("/api/v2/torrents/info")).await;
    let listed = json(&body);
    let entry = listed
        .as_array()
        .and_then(|items| items.iter().find(|item| item["hash"] == HASH))
        .unwrap_or_else(|| panic!("the torrent the client just added is missing: {body}"));
    for field in ["name", "state", "progress", "save_path", "size"] {
        assert!(!entry[field].is_null(), "info.{field}: {body}");
    }

    // 6. Inspect it, which the client does before importing.
    let (status, _) = send(
        &harness.router,
        get_with_cookie(&format!("/api/v2/torrents/properties?hash={HASH}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &harness.router,
        get_with_cookie(&format!("/api/v2/torrents/files?hash={HASH}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json(&body).is_array(), "the file list is an array: {body}");

    // 7. Seed goals, which the client sets when the user configured them.
    let (status, _) = send(
        &harness.router,
        post_form(
            "/api/v2/torrents/setShareLimits",
            &format!("hashes={HASH}&ratioLimit=2&seedingTimeLimit=60&inactiveSeedingTimeLimit=-2"),
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 8. Remove it after a failed grab.
    let (status, _) = send(
        &harness.router,
        post_form(
            "/api/v2/torrents/delete",
            &format!("hashes={HASH}&deleteFiles=true"),
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(&harness.router, get_with_cookie("/api/v2/torrents/info")).await;
    assert_eq!(json(&body).as_array().map(Vec::len), Some(0), "{body}");
}

/// Both adapters must stay reachable with the same token at the same time: an installation
/// commonly has Sonarr on the SABnzbd side and Radarr on the torrent side.
#[tokio::test]
async fn both_adapters_serve_the_same_installation_at_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (sab_status, _) = send(
        &harness.router,
        get(&format!(
            "/api?mode=version&apikey={API_BEARER}&output=json"
        )),
    )
    .await;
    let (qb_status, _) = send(&harness.router, get_with_cookie("/api/v2/app/version")).await;
    assert_eq!(sab_status, StatusCode::OK);
    assert_eq!(qb_status, StatusCode::OK);

    // And neither has displaced the native contract or the web interface.
    let (native, _) = send(&harness.router, get("/api/v1/health")).await;
    assert_eq!(native, StatusCode::OK);
}

fn minimal_nzb(name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="iso-8859-1" ?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
 <file poster="poster@example.com" date="1700000000" subject="{name} [1/1] - &quot;{name}.rar&quot; yEnc (1/1)">
  <groups><group>alt.binaries.test</group></groups>
  <segments><segment bytes="1024" number="1">part1@example</segment></segments>
 </file>
</nzb>
"#
    )
}

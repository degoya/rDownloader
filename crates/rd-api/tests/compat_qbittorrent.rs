//! The qBittorrent-compatible surface, exercised the way an automation client uses it:
//! log in, read the app version, poll `torrents/info`, act on a hash.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{API_BEARER, READ_BEARER, auth_harness};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<(String, String)>,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, format!("SID={cookie}"));
    }
    let request = match body {
        Some((content_type, payload)) => builder
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(payload))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
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

async fn login(router: &axum::Router, password: &str) -> (StatusCode, String) {
    call(
        router,
        "POST",
        "/api/v2/auth/login",
        None,
        Some((
            "application/x-www-form-urlencoded".to_owned(),
            format!("username=admin&password={password}"),
        )),
    )
    .await
}

#[tokio::test]
async fn a_valid_token_logs_in_and_a_wrong_one_does_not() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // qBittorrent answers 200 with a body of `Ok.` or `Fails.`; clients compare the body,
    // so a status code alone would not tell them anything.
    let (status, body) = login(&harness.router, API_BEARER).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "Ok.");

    for password in ["", "wrong", READ_BEARER] {
        let (status, body) = login(&harness.router, password).await;
        assert_eq!(status, StatusCode::OK, "{password}");
        assert_eq!(body, "Fails.", "{password} was accepted");
    }
}

/// The cookie is a handle, not the credential.
///
/// It used to be the `api:*` bearer itself, set at `Path=/`: every path on the origin could
/// read a full API credential, and a plain-http hop carried it in clear text. What a client
/// sends is unchanged — a bearer in the `SID` cookie is still accepted — but what this service
/// hands out is a value that means nothing outside the process that minted it, which is why
/// the restart below has to refuse it.
#[tokio::test]
async fn the_session_cookie_is_an_opaque_handle_and_does_not_survive_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let request = Request::builder()
        .method("POST")
        .uri("/api/v2/auth/login")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("password={API_BEARER}")))
        .expect("request");
    let response = harness
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("response");
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("set-cookie")
        .to_owned();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Strict"), "{cookie}");
    assert!(
        !cookie.contains(API_BEARER),
        "the cookie still carries the bearer itself: {cookie}"
    );
    let handle = cookie
        .split(';')
        .next()
        .and_then(|part| part.trim().strip_prefix("SID="))
        .expect("a SID value")
        .to_owned();

    // The handle works, so the client that logged in is logged in.
    let (status, body) = call(
        &harness.router,
        "GET",
        "/api/v2/app/version",
        Some(&handle),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A second router over the same database — a restart — refuses it: the handle lived in
    // the process that minted it and cannot be replayed into the next one. The client logs in
    // again, which is what real qBittorrent makes it do.
    let restarted = auth_harness(directory.path()).await;
    let (status, _) = call(
        &restarted.router,
        "GET",
        "/api/v2/app/version",
        Some(&handle),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The credential behind it is untouched: the token still works, here and after a restart.
    let (status, body) = call(
        &restarted.router,
        "GET",
        "/api/v2/app/version",
        Some(API_BEARER),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with('v'), "{body}");
}

/// Logging out invalidates the handle here, not only in the browser.
#[tokio::test]
async fn logging_out_stops_the_handle_working() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let request = Request::builder()
        .method("POST")
        .uri("/api/v2/auth/login")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("password={API_BEARER}")))
        .expect("request");
    let response = harness
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("response");
    let handle = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie| cookie.split(';').next())
        .and_then(|part| part.trim().strip_prefix("SID="))
        .expect("a SID value")
        .to_owned();

    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/auth/logout",
        Some(&handle),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call(
        &harness.router,
        "GET",
        "/api/v2/app/version",
        Some(&handle),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a handle kept after logging out still worked"
    );
}

#[tokio::test]
async fn an_unauthenticated_call_is_forbidden_not_broken() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // 403 is what a qBittorrent client reads as "log in again"; anything else reads as an
    // outage and stops it from retrying.
    for uri in [
        "/api/v2/app/version",
        "/api/v2/app/webapiVersion",
        "/api/v2/app/preferences",
        "/api/v2/torrents/info",
        "/api/v2/torrents/categories",
    ] {
        let (status, _) = call(&harness.router, "GET", uri, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        let (status, _) = call(&harness.router, "GET", uri, Some(READ_BEARER), None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} with a read-only token"
        );
    }
}

#[tokio::test]
async fn the_app_endpoints_answer_what_a_client_branches_on() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (_, version) = call(
        &harness.router,
        "GET",
        "/api/v2/app/version",
        Some(API_BEARER),
        None,
    )
    .await;
    assert!(version.starts_with('v'), "{version}");

    let (_, api_version) = call(
        &harness.router,
        "GET",
        "/api/v2/app/webapiVersion",
        Some(API_BEARER),
        None,
    )
    .await;
    assert!(
        api_version.split('.').count() >= 2,
        "clients parse this as a version: {api_version}"
    );

    let (_, preferences) = call(
        &harness.router,
        "GET",
        "/api/v2/app/preferences",
        Some(API_BEARER),
        None,
    )
    .await;
    let preferences: serde_json::Value = serde_json::from_str(&preferences).expect("json");
    assert!(
        preferences["save_path"].is_string(),
        "a client reads save_path to find finished files: {preferences}"
    );
}

#[tokio::test]
async fn an_empty_torrent_list_is_an_array_not_an_object() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (status, body) = call(
        &harness.router,
        "GET",
        "/api/v2/torrents/info",
        Some(API_BEARER),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(parsed.as_array().map(Vec::len), Some(0), "{body}");

    let (_, categories) = call(
        &harness.router,
        "GET",
        "/api/v2/torrents/categories",
        Some(API_BEARER),
        None,
    )
    .await;
    let parsed: serde_json::Value = serde_json::from_str(&categories).expect("json");
    assert!(parsed.is_object(), "categories are a map: {categories}");
}

#[tokio::test]
async fn an_unknown_hash_is_reported_as_missing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    for uri in [
        "/api/v2/torrents/properties?hash=abcdef0123456789abcdef0123456789abcdef01",
        "/api/v2/torrents/files?hash=abcdef0123456789abcdef0123456789abcdef01",
    ] {
        let (status, _) = call(&harness.router, "GET", uri, Some(API_BEARER), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn acting_on_an_unknown_hash_changes_nothing_and_still_succeeds() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // A client deletes what it thinks it added; a hash we never had must not be an error,
    // or the client retries the same removal forever.
    for uri in [
        "/api/v2/torrents/delete?hashes=abcdef0123456789abcdef0123456789abcdef01&deleteFiles=true",
        "/api/v2/torrents/pause?hashes=abcdef0123456789abcdef0123456789abcdef01",
        "/api/v2/torrents/resume?hashes=all",
    ] {
        let (status, _) = call(&harness.router, "POST", uri, Some(API_BEARER), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn adding_without_a_torrent_or_magnet_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    const BOUNDARY: &str = "----rdtest";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"category\"\r\n\r\ntv\r\n--{BOUNDARY}--\r\n"
    );
    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/torrents/add",
        Some(API_BEARER),
        Some((format!("multipart/form-data; boundary={BOUNDARY}"), body)),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn a_non_magnet_url_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // Following an `http` URL supplied by the caller would let an API key reach whatever
    // the service host can reach; only magnets are accepted here.
    const BOUNDARY: &str = "----rdtest";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"urls\"\r\n\r\nhttp://127.0.0.1:1/evil.torrent\r\n--{BOUNDARY}--\r\n"
    );
    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/torrents/add",
        Some(API_BEARER),
        Some((format!("multipart/form-data; boundary={BOUNDARY}"), body)),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn a_magnet_is_queued_and_appears_with_a_hash() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    const BOUNDARY: &str = "----rdtest";
    const HASH: &str = "abcdef0123456789abcdef0123456789abcdef01";
    let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=Example.Release");
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"urls\"\r\n\r\n{magnet}\r\n--{BOUNDARY}--\r\n"
    );
    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/torrents/add",
        Some(API_BEARER),
        Some((format!("multipart/form-data; boundary={BOUNDARY}"), body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The client polls for the hash it computed from the magnet it just sent, long before
    // the engine has metadata. The torrent has to be visible under that hash immediately or
    // the client concludes the add failed.
    let (status, body) = call(
        &harness.router,
        "GET",
        "/api/v2/torrents/info",
        Some(API_BEARER),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{body}");
    assert_eq!(listed[0]["hash"], HASH, "{body}");
    assert_eq!(listed[0]["name"], "Example.Release", "{body}");
    assert!(listed[0]["save_path"].is_string(), "{body}");

    // And it can be acted on by that hash straight away.
    let (status, _) = call(
        &harness.router,
        "POST",
        &format!("/api/v2/torrents/delete?hashes={HASH}"),
        Some(API_BEARER),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = call(
        &harness.router,
        "GET",
        "/api/v2/torrents/info",
        Some(API_BEARER),
        None,
    )
    .await;
    let listed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(listed.as_array().map(Vec::len), Some(0), "{body}");
}

#[tokio::test]
async fn changing_a_file_selection_needs_a_known_torrent_and_a_valid_body() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // An unknown hash is reported as missing rather than silently accepted, and a request
    // without file indices is a bad request — a client that mis-sends must find out.
    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/torrents/filePrio?hash=abcdef0123456789abcdef0123456789abcdef01",
        Some(API_BEARER),
        Some((
            "application/x-www-form-urlencoded".to_owned(),
            "id=0&priority=0".to_owned(),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/torrents/filePrio",
        None,
        Some((
            "application/x-www-form-urlencoded".to_owned(),
            "id=0&priority=0".to_owned(),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "unauthenticated");
}

/// Every route in this adapter refuses an unauthenticated call — all of them, not the ones
/// somebody remembered to write a test for.
///
/// The routes used to authenticate themselves: each handler opened with the same three lines
/// calling `guard`, twenty-odd times over. Forgetting one left that route open, answering
/// normally, with nothing anywhere to say so — and `compat::routes()` carried no layer, so
/// there was no backstop either. Authentication is now a layer on the guarded sub-router, and
/// a handler added to it is protected because of where it is rather than because of what it
/// remembers to call.
///
/// The route list is read out of the source rather than restated here. A hand-kept copy would
/// have exactly the failure mode being fixed: someone adds a route and does not add it to the
/// list, and the test goes on passing.
#[tokio::test]
async fn every_route_in_the_adapter_refuses_an_unauthenticated_call() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;

    let source = include_str!("../src/compat/qbittorrent/mod.rs");
    let guarded = source
        .split_once("fn guarded_routes()")
        .expect("the guarded router exists")
        .1;
    let mut checked = 0_usize;
    for fragment in guarded.split(".route(").skip(1) {
        let Some(path) = fragment
            .split_once('"')
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(path, _)| path)
        else {
            continue;
        };
        assert!(path.starts_with("/api/v2/"), "unexpected route {path}");
        for method in ["GET", "POST"] {
            let (status, body) = call(&harness.router, method, path, None, None).await;
            assert!(
                matches!(
                    status,
                    StatusCode::FORBIDDEN | StatusCode::METHOD_NOT_ALLOWED
                ),
                "{method} {path} answered {status} without a credential: {body}"
            );
        }
        checked += 1;
    }
    // The exact count, not a floor: a route removed from the adapter should be noticed too,
    // and a scrape that silently stops finding them would otherwise read as a clean pass.
    assert_eq!(
        checked, 18,
        "the adapter's route count changed; update this number after checking the new route \
         is inside guarded_routes() and not beside it"
    );
}

/// The two routes that hand out a credential must not require one — and nothing else may
/// join them.
///
/// The test above proves the guarded router is guarded. This one closes the other half: a
/// route added to the *public* router by mistake would never appear there, so the count would
/// stay at eighteen and the suite would go on passing while the route sat open.
#[tokio::test]
async fn only_the_login_routes_are_reachable_without_a_credential() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;

    let source = include_str!("../src/compat/qbittorrent/mod.rs");
    let public = source
        .split_once("fn public_routes()")
        .expect("the public router exists")
        .1
        .split_once("fn guarded_routes()")
        .expect("the guarded router follows it")
        .0;
    let paths: Vec<&str> = public
        .split(".route(")
        .skip(1)
        .filter_map(|fragment| {
            fragment
                .split_once('"')
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(path, _)| path)
        })
        .collect();
    assert_eq!(
        paths,
        vec!["/api/v2/auth/login", "/api/v2/auth/logout"],
        "a route joined the unauthenticated half of the adapter"
    );

    for path in paths {
        let (status, _) = call(&harness.router, "POST", path, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
}

const MAGNET_BOUNDARY: &str = "----rdtest";

/// The stored and effective seeding policy of one download, through the native API.
async fn seeding_policy(router: &axum::Router, id: &str) -> serde_json::Value {
    let (status, body) = common::get_with_bearer(
        router,
        &format!("/api/v1/downloads/{id}/torrent/seeding"),
        API_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Queues a magnet the way a client does, with the category it configured.
async fn add_magnet(router: &axum::Router, hash: &str, category: Option<&str>) {
    let magnet = format!("magnet:?xt=urn:btih:{hash}&dn=Example.Release");
    let mut body = format!(
        "--{MAGNET_BOUNDARY}\r\nContent-Disposition: form-data; name=\"urls\"\r\n\r\n{magnet}\r\n"
    );
    if let Some(category) = category {
        body.push_str(&format!(
            "--{MAGNET_BOUNDARY}\r\nContent-Disposition: form-data; name=\"category\"\r\n\r\n{category}\r\n"
        ));
    }
    body.push_str(&format!("--{MAGNET_BOUNDARY}--\r\n"));
    let (status, reply) = call(
        router,
        "POST",
        "/api/v2/torrents/add",
        Some(API_BEARER),
        Some((
            format!("multipart/form-data; boundary={MAGNET_BOUNDARY}"),
            body,
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
}

async fn info(router: &axum::Router, query: &str) -> serde_json::Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/api/v2/torrents/info{query}"),
        Some(API_BEARER),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str(&body).expect("json")
}

/// Creates a category through the native API, with the storage root it needs — a bare test
/// router has none.
async fn create_category(router: &axum::Router, directory: &std::path::Path, name: &str) {
    let path = directory.join("downloads");
    std::fs::create_dir_all(&path).expect("downloads");
    let (status, root) = common::post_with_bearer(
        router,
        "/api/v1/storage-roots",
        API_BEARER,
        serde_json::json!({ "name": "Primary", "path": path.to_string_lossy(), "is_default": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{root}");
    let root = root["id"].clone();
    let (status, created) = common::post_with_bearer(
        router,
        "/api/v1/categories",
        API_BEARER,
        serde_json::json!({
            "name": name,
            "color": "#336699",
            "storage_root_id": root,
            "relative_path": name,
            "is_default": false,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}

/// Sonarr and Radarr never read the unfiltered list: each polls `torrents/info` narrowed to
/// the category it configured and imports what it finds there. A filter that hides the
/// torrents queued under that very category leaves every grab waiting forever.
#[tokio::test]
async fn the_category_filter_finds_the_torrents_queued_under_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    create_category(&harness.router, directory.path(), "tv-sonarr").await;
    const HASH: &str = "abcdef0123456789abcdef0123456789abcdef01";
    add_magnet(&harness.router, HASH, Some("tv-sonarr")).await;

    let listed = info(&harness.router, "?category=tv-sonarr").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["hash"], HASH, "{listed}");
    assert_eq!(
        listed[0]["category"], "tv-sonarr",
        "the category is reported back, so the client can trust its own filter: {listed}"
    );

    let listed = info(&harness.router, "?category=movies-radarr").await;
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(0),
        "another tool's category shows nothing of this one: {listed}"
    );
    let listed = info(&harness.router, "").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
}

/// A torrent queued without a category is listed under the empty category, which is what
/// qBittorrent answers for `category=` — and nothing else.
#[tokio::test]
async fn a_torrent_without_a_category_is_listed_under_the_empty_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
    add_magnet(&harness.router, HASH, None).await;

    let listed = info(&harness.router, "?category=").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["category"], "", "{listed}");
    let listed = info(&harness.router, "?category=tv-sonarr").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(0), "{listed}");
}

/// A path the adapter does not serve is `404`, as qBittorrent answers it. Falling through to
/// the web interface answered `200` with `index.html`, which a client parsing JSON reads as a
/// server that is up and broken rather than a feature that is missing.
#[tokio::test]
async fn an_unknown_endpoint_is_not_found_rather_than_the_web_interface() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    for (method, path) in [
        ("GET", "/api/v2/torrents/nonsense"),
        ("POST", "/api/v2/app/nonsense"),
        ("GET", "/api/v2/sync/maindata"),
    ] {
        let (status, body) = call(&harness.router, method, path, Some(API_BEARER), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {body}");
        assert!(
            !body.contains("<html"),
            "{method} {path} served the web interface"
        );
    }
}

/// `ratioLimit=-2` is qBittorrent's "seed without a ratio limit"; `-1` leaves the global
/// setting in charge. The two must land as different overrides, or a client that asked for
/// unlimited seeding gets whatever the global ratio happens to be.
#[tokio::test]
async fn a_share_limit_of_minus_two_removes_the_ratio_stop() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    const HASH: &str = "fedcba9876543210fedcba9876543210fedcba98";
    add_magnet(&harness.router, HASH, None).await;
    let (status, downloads) =
        common::get_with_bearer(&harness.router, "/api/v1/downloads", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{downloads}");
    let id = downloads[0]["id"]
        .as_str()
        .expect("the queued download")
        .to_owned();

    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/torrents/setShareLimits",
        Some(API_BEARER),
        Some((
            "application/x-www-form-urlencoded".to_owned(),
            format!("hashes={HASH}&ratioLimit=-2&seedingTimeLimit=-2&inactiveSeedingTimeLimit=-2"),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let policy = seeding_policy(&harness.router, &id).await;
    assert_eq!(
        policy["torrent_override"]["ratio_milli"], 0,
        "a ratio of zero is the stored form of no ratio stop: {policy}"
    );
    assert_eq!(policy["effective"]["ratio"], 0.0, "{policy}");
    assert_eq!(policy["effective"]["ratio_source"], "torrent", "{policy}");

    let (status, _) = call(
        &harness.router,
        "POST",
        "/api/v2/torrents/setShareLimits",
        Some(API_BEARER),
        Some((
            "application/x-www-form-urlencoded".to_owned(),
            format!("hashes={HASH}&ratioLimit=-1&seedingTimeLimit=-1&inactiveSeedingTimeLimit=-1"),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let policy = seeding_policy(&harness.router, &id).await;
    assert!(
        policy["torrent_override"]["ratio_milli"].is_null(),
        "-1 hands the ratio back to the global setting: {policy}"
    );
    assert_eq!(policy["effective"]["ratio_source"], "global", "{policy}");
}

/// The `SID` cookie follows the same policy as the native session cookie.
///
/// It carries an opaque handle rather than the bearer now, but a handle is still a live
/// session: a missing `Secure` on an https deployment sends it over any plain-http hop, and
/// `Path=/` hands it to every path on the origin rather than only the one the service is
/// mounted at.
#[tokio::test]
async fn the_session_cookie_follows_the_deployment_cookie_policy() {
    for (external, expect_secure, expect_path) in [
        ("https://rd.example.test", true, "/"),
        ("http://rd.example.test/downloads", false, "/downloads"),
    ] {
        let directory = tempfile::tempdir().expect("tempdir");
        let harness = common::test_harness(directory.path()).await;
        let (status, settings) = common::get_json(&harness.router, "/api/v1/settings").await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        let mut settings = settings;
        // The harness switches the login off on the service rather than through this
        // document, so saving it unchanged would switch it back on.
        settings["admin_login_disabled"] = serde_json::Value::Bool(true);
        settings["external_url"] = serde_json::Value::String(external.to_owned());
        let (status, body) = common::put_json(&harness.router, "/api/v1/settings", settings).await;
        assert_eq!(status, StatusCode::OK, "{external}: {body}");

        let request = Request::builder()
            .method("POST")
            .uri("/api/v2/auth/login")
            .header(header::HOST, "127.0.0.1:8710")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!("password={API_BEARER}")))
            .expect("request");
        let response = harness
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("response");
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("set-cookie")
            .to_owned();
        assert_eq!(
            cookie.contains("; Secure"),
            expect_secure,
            "{external} produced `{cookie}`"
        );
        let path = cookie
            .split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix("Path="))
            .expect("a path attribute");
        assert_eq!(path, expect_path, "{external} produced `{cookie}`");
    }
}

/// The compat login is metered like the native one.
///
/// Every attempt costs an unauthenticated caller a SHA-256 and a database lookup. Leaving this
/// door free while the native one is counted only tells an attacker which door to use.
#[tokio::test]
async fn repeated_wrong_tokens_are_throttled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let mut banned = None;
    for attempt in 0..12 {
        let (status, body) = login(&harness.router, "wrong-token").await;
        if status == StatusCode::FORBIDDEN {
            banned = Some(attempt);
            break;
        }
        assert_eq!(status, StatusCode::OK, "attempt {attempt}: {body}");
        assert_eq!(body, "Fails.", "attempt {attempt}");
    }
    let attempt = banned.expect("guessing was never throttled");
    assert!(attempt >= 5, "throttled after only {attempt} attempts");

    // The real token is refused too while the lockout stands: the limiter runs before the
    // credential is looked at, so a locked-out address learns nothing by guessing correctly.
    let (status, _) = login(&harness.router, API_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// RD-120-68: a torrent a client deletes leaves the engine session too, found by the info
/// hash on the row since nothing was added to the engine in this process.
#[tokio::test]
async fn a_deleted_torrent_leaves_the_engine_session() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
    add_magnet(&harness.router, HASH, None).await;
    common::persist_torrent_session(directory.path(), HASH);

    let (status, _) = call(
        &harness.router,
        "POST",
        &format!("/api/v2/torrents/delete?hashes={HASH}"),
        Some(API_BEARER),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        info(&harness.router, "").await.as_array().map(Vec::len),
        Some(0)
    );
    assert!(common::persisted_torrents(directory.path()).is_empty());
}

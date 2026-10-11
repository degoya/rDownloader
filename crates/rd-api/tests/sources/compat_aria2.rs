//! The aria2 JSON-RPC surface (RD-1240-11), driven the way AriaNg drives it.
//!
//! The requests are the shapes AriaNg sends: the version on connect, the global figures and
//! the three lists with the keys it shows, a `system.multicall` from its toolbar. They run
//! against the login-enabled harness, because the secret is the whole access story of this
//! adapter, and a parked scheduler, so a queued link stays where the test put it.

use crate::common;

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use common::{API_BEARER, CAPTURE_BEARER, READ_BEARER};
use serde_json::{Value, json};

async fn harness(directory: &std::path::Path) -> common::Harness {
    common::harness(directory, common::Options::default().login().parked()).await
}

/// Switches the adapter on in the stored document, as saving the settings page does.
///
/// Written to the store rather than through `PUT /api/v1/settings`: saving applies the
/// document live, and its `max_active_files` would wake the parked scheduler.
async fn switch_on(database: &rd_db::Database) {
    let mut blob = database
        .get_setting(rd_db::SERVICE_SETTINGS_KEY)
        .await
        .expect("settings")
        .unwrap_or_else(|| json!({}));
    blob["aria2_rpc_enabled"] = Value::Bool(true);
    database
        .set_setting(rd_db::SERVICE_SETTINGS_KEY.to_owned(), blob)
        .await
        .expect("switched on");
}

/// One `POST /jsonrpc`, returning the status and the decoded body.
async fn rpc(router: &axum::Router, body: &Value) -> (StatusCode, Value) {
    common::send(
        router,
        common::request_to("POST", "/jsonrpc")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await
}

/// One call with `secret` as its `token:` parameter.
fn call(method: &str, secret: &str, params: &[Value]) -> Value {
    let mut all = vec![json!(format!("token:{secret}"))];
    all.extend_from_slice(params);
    json!({ "jsonrpc": "2.0", "method": method, "id": "ariang", "params": all })
}

async fn result(router: &axum::Router, method: &str, params: &[Value]) -> Value {
    let (status, body) = rpc(router, &call(method, API_BEARER, params)).await;
    assert_eq!(status, StatusCode::OK, "{method}: {body}");
    assert_eq!(body["id"], "ariang", "{method}: {body}");
    body["result"].clone()
}

/// The CORS preflight a page served elsewhere sends before its first call.
fn preflight() -> axum::http::Request<Body> {
    common::request_to("OPTIONS", "/jsonrpc")
        .header(header::ORIGIN, "https://ariang.example.test")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
        .body(Body::empty())
        .expect("request")
}

#[tokio::test]
async fn switched_off_the_path_does_not_exist() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;

    let (status, _) = rpc(&harness.router, &call("aria2.getVersion", API_BEARER, &[])).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "off by default");
    let (status, _, _) = common::send_raw(&harness.router, preflight()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the preflight too");
}

#[tokio::test]
async fn a_missing_wrong_or_narrow_secret_is_refused_in_aria2_s_words() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    switch_on(&harness.database).await;

    let version = |secret: &str| call("aria2.getVersion", secret, &[]);
    let mixed = json!([version(API_BEARER), version("another-secret")]);
    for body in [
        json!({ "jsonrpc": "2.0", "method": "aria2.getVersion", "id": "ariang" }),
        version(""),
        version("wrong"),
        version(READ_BEARER),
        version(CAPTURE_BEARER),
    ] {
        let (status, answer) = rpc(&harness.router, &body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}: {answer}");
        // The word AriaNg looks for to say "wrong secret".
        assert_eq!(
            answer["error"]["message"], "Unauthorized",
            "{body}: {answer}"
        );
        assert_eq!(answer["id"], "ariang", "{body}");
    }
    // One request, two secrets: refused whole, every call in its slot.
    let (status, answer) = rpc(&harness.router, &mixed).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer[1]["error"]["message"], "Unauthorized", "{answer}");
}

#[tokio::test]
async fn ariang_s_connect_and_poll_are_answered() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    switch_on(&harness.database).await;
    let router = &harness.router;

    let version = result(router, "aria2.getVersion", &[]).await;
    assert_eq!(version["version"], "1.37.0", "{version}");
    assert!(version["enabledFeatures"].is_array(), "{version}");

    let methods = result(router, "system.listMethods", &[]).await;
    for method in ["aria2.addUri", "aria2.tellStatus", "system.multicall"] {
        assert!(
            methods
                .as_array()
                .is_some_and(|all| all.contains(&json!(method))),
            "{method}: {methods}"
        );
    }

    let stat = result(router, "aria2.getGlobalStat", &[]).await;
    for field in [
        "downloadSpeed",
        "uploadSpeed",
        "numActive",
        "numWaiting",
        "numStopped",
    ] {
        assert!(
            stat[field].is_string(),
            "aria2 sends {field} as a string: {stat}"
        );
    }

    let keys = json!(["gid", "totalLength", "completedLength", "status", "files"]);
    assert_eq!(
        result(router, "aria2.tellActive", std::slice::from_ref(&keys)).await,
        json!([])
    );
    for method in ["aria2.tellWaiting", "aria2.tellStopped"] {
        let list = result(router, method, &[json!(0), json!(1000), keys.clone()]).await;
        assert_eq!(list, json!([]), "{method}");
    }

    // A method this subset does not have is JSON-RPC's "Method not found", not a silent
    // success and no server fault (RD-1240-28).
    let (status, answer) = rpc(router, &call("aria2.changePosition", API_BEARER, &[])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["error"]["code"], -32601, "{answer}");
    assert_eq!(
        answer["error"]["message"],
        "Method not found: aria2.changePosition"
    );
}

/// Walks a queued download through `states`, as the scheduler would.
async fn walk(harness: &common::Harness, gid: &str, states: &[rd_core::DownloadState]) {
    let downloads = harness.database.list_downloads().await.expect("downloads");
    let id = downloads
        .iter()
        .find(|download| download.id.to_string().replace('-', "").ends_with(gid))
        .expect("the GID's download")
        .id;
    for state in states {
        harness
            .database
            .transition_download(id, *state)
            .await
            .expect("transition");
    }
}

/// What AriaNg sends from its stopped list, its task page and its settings pages: these were
/// "No such method" with a `500` in the live test of 1.24 (RD-1240-28).
#[tokio::test]
async fn ariang_s_stopped_list_task_page_and_settings_are_answered() {
    use rd_core::DownloadState::{Completed, Downloading, Failed, Resolving, Verifying};
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    switch_on(&harness.database).await;
    let router = &harness.router;
    let mut gids = Vec::new();
    for name in ["finished.bin", "failed.bin", "waiting.bin"] {
        let added = result(
            router,
            "aria2.addUri",
            &[
                json!([format!("https://example.test/{name}")]),
                json!({ "out": name }),
            ],
        )
        .await;
        gids.push(added.as_str().expect("a GID").to_owned());
    }
    let (finished, failed, waiting) = (&gids[0], &gids[1], &gids[2]);
    walk(
        &harness,
        finished,
        &[Resolving, Downloading, Verifying, Completed],
    )
    .await;
    walk(&harness, failed, &[Resolving, Downloading, Failed]).await;

    // The task page.
    let files = result(router, "aria2.getFiles", &[json!(finished)]).await;
    assert_eq!(files[0]["index"], "1", "{files}");
    assert!(
        files[0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("finished.bin")),
        "{files}"
    );
    let uris = result(router, "aria2.getUris", &[json!(finished)]).await;
    assert_eq!(
        uris[0]["uri"], "https://example.test/finished.bin",
        "{uris}"
    );
    assert_eq!(
        result(router, "aria2.getPeers", &[json!(finished)]).await,
        json!([])
    );
    let option = result(router, "aria2.getOption", &[json!(finished)]).await;
    assert_eq!(option["out"], "finished.bin", "{option}");
    assert!(option["dir"].is_string(), "{option}");
    // Accepted and without effect: the service's own settings decide.
    let limit = json!({ "max-download-limit": "1K" });
    assert_eq!(
        result(
            router,
            "aria2.changeOption",
            &[json!(finished), limit.clone()]
        )
        .await,
        json!("OK")
    );
    let (status, answer) = rpc(
        router,
        &call("aria2.getOption", API_BEARER, &[json!("2089b05ecca3d829")]),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{answer}");
    assert_eq!(answer["error"]["code"], 1, "{answer}");

    // The settings pages.
    let global = result(router, "aria2.getGlobalOption", &[]).await;
    for key in [
        "dir",
        "max-concurrent-downloads",
        "max-overall-download-limit",
    ] {
        assert!(global[key].is_string(), "{key}: {global}");
    }
    assert_eq!(
        result(router, "aria2.changeGlobalOption", &[limit]).await,
        json!("OK")
    );

    // The stopped list: "Remove Task" takes a stopped download only, "Clear Stopped Tasks"
    // every one of them.
    let (status, answer) = rpc(
        router,
        &call("aria2.removeDownloadResult", API_BEARER, &[json!(waiting)]),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{answer}");
    assert_eq!(
        answer["error"]["message"],
        format!("Could not remove download result of GID#{waiting}")
    );
    assert_eq!(
        result(router, "aria2.removeDownloadResult", &[json!(finished)]).await,
        json!("OK")
    );
    let left = harness.database.list_downloads().await.expect("downloads");
    assert_eq!(left.len(), 2, "the finished one is gone");
    assert_eq!(
        result(router, "aria2.purgeDownloadResult", &[]).await,
        json!("OK")
    );
    let left = harness.database.list_downloads().await.expect("downloads");
    assert_eq!(left.len(), 1, "only the waiting one stays");
    assert_eq!(left[0].file_name, "waiting.bin");
}

#[tokio::test]
async fn a_link_added_by_add_uri_is_found_paused_resumed_and_removed_by_its_gid() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    switch_on(&harness.database).await;
    let router = &harness.router;

    let added = result(
        router,
        "aria2.addUri",
        &[
            json!(["https://example.test/releases/file.iso"]),
            json!({ "out": "renamed.iso" }),
        ],
    )
    .await;
    let gid = added.as_str().expect("a GID").to_owned();
    assert_eq!(gid.len(), 16, "{gid}");
    let downloads = harness.database.list_downloads().await.expect("downloads");
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0].file_name, "renamed.iso");

    let status = result(router, "aria2.tellStatus", &[json!(gid)]).await;
    assert_eq!(status["gid"], gid.as_str(), "{status}");
    assert_eq!(status["status"], "waiting", "{status}");
    assert_eq!(
        status["files"][0]["uris"][0]["uri"],
        "https://example.test/releases/file.iso"
    );
    let waiting = result(
        router,
        "aria2.tellWaiting",
        &[json!(0), json!(10), json!(["gid"])],
    )
    .await;
    assert_eq!(waiting, json!([{ "gid": gid }]), "only the asked key");

    assert_eq!(
        result(router, "aria2.pause", &[json!(gid)]).await,
        json!(gid)
    );
    let status = result(router, "aria2.tellStatus", &[json!(gid), json!(["status"])]).await;
    assert_eq!(status, json!({ "status": "paused" }));
    assert_eq!(
        result(router, "aria2.unpause", &[json!(gid)]).await,
        json!(gid)
    );
    let status = result(router, "aria2.tellStatus", &[json!(gid), json!(["status"])]).await;
    assert_eq!(status, json!({ "status": "waiting" }));

    assert_eq!(
        result(router, "aria2.remove", &[json!(gid)]).await,
        json!(gid)
    );
    assert!(
        harness
            .database
            .list_downloads()
            .await
            .expect("downloads")
            .is_empty()
    );
    let (status, answer) = rpc(router, &call("aria2.tellStatus", API_BEARER, &[json!(gid)])).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{answer}");
    assert_eq!(
        answer["error"]["message"],
        format!("GID {gid} is not found")
    );
}

#[tokio::test]
async fn a_multicall_and_a_batch_answer_each_call_in_its_slot() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    switch_on(&harness.database).await;
    let router = &harness.router;
    let secret = format!("token:{API_BEARER}");

    // `pause: "true"` adds it paused, as AriaNg's "add paused" does.
    let gid = result(
        router,
        "aria2.addUri",
        &[
            json!(["https://example.test/a.bin"]),
            json!({ "pause": "true" }),
        ],
    )
    .await;
    let paused = result(
        router,
        "aria2.tellStatus",
        &[gid.clone(), json!(["status"])],
    )
    .await;
    assert_eq!(paused, json!({ "status": "paused" }));

    // AriaNg's toolbar: the secret inside each call, one of them for a task that is gone.
    let multicall = json!({
        "jsonrpc": "2.0", "method": "system.multicall", "id": "ariang",
        "params": [[
            { "methodName": "aria2.unpause", "params": [secret, gid] },
            { "methodName": "aria2.forcePause", "params": [secret, "2089b05ecca3d829"] },
        ]],
    });
    let (status, answer) = rpc(router, &multicall).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["result"][0], json!([gid]), "{answer}");
    assert_eq!(answer["result"][1]["code"], 1, "{answer}");

    let batch = json!([
        call("aria2.getVersion", API_BEARER, &[]),
        { "jsonrpc": "2.0", "id": 7 },
        call("aria2.tellActive", API_BEARER, &[json!(["gid"])]),
    ]);
    let (status, answer) = rpc(router, &batch).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer[0]["result"]["version"], "1.37.0", "{answer}");
    assert_eq!(answer[1]["error"]["code"], -32600, "{answer}");
    assert_eq!(answer[1]["id"], 7, "{answer}");
    assert!(answer[2]["result"].is_array(), "{answer}");
}

#[tokio::test]
async fn a_page_served_elsewhere_may_call_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    switch_on(&harness.database).await;

    // AriaNg is a page of its own origin: without these answers the browser withholds every
    // reply from it. The secret travels in the body, so nothing ambient comes along.
    let (status, headers, _) = common::send_raw(&harness.router, preflight()).await;
    assert!(status.is_success(), "{status}");
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("*")
    );
    let (status, headers, _) = common::send_raw(
        &harness.router,
        common::request_to("POST", "/jsonrpc")
            .header(header::ORIGIN, "https://ariang.example.test")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                call("aria2.getVersion", API_BEARER, &[]).to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS));
}

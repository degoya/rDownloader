//! A container handed in as base64 in a JSON body (RD-120-31).
//!
//! Every case runs against a recorded file from `testfile/`, and the ones that matter most
//! run it twice — once as the multipart upload a browser sends, once as JSON, each into a
//! fresh installation — and require the same answer. That is what "no second intake path"
//! means in a test: not that the code looks shared, but that the two bodies cannot be told
//! apart by what they produce.
//!
//! A DLC is opened by an online service. The service's answer for `testfile/test.dlc` was
//! recorded once and is served here from loopback, so the whole path runs and nothing leaves
//! the machine.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use common::{auth_harness, get_json, put_json, send, test_router};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const DLC: &[u8] = include_bytes!("../../../testfile/test.dlc");
const TORRENT: &[u8] = include_bytes!("../../../testfile/big-buck-bunny.torrent");
const NZB: &[u8] = include_bytes!("../../../testfile/sabnzbd-test-download-100MB.nzb");

/// What `service.jdownloader.org/dlcrypt/service.php` answered for `test.dlc`'s key blob
/// (`srcType=dlc`, `destType=pylo`), recorded on 2026-09-23.
const DLC_SERVICE_ANSWER: &str = "<rc>y4hx8S5sJrmfs1An9K8ZYw==</rc>";

fn multipart(file_name: &str, content: &[u8]) -> (String, Vec<u8>) {
    let boundary = "rdcontainerjsonboundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn post(uri: &str, content_type: &str, body: impl Into<Body>) -> Request<Body> {
    Request::post(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, content_type)
        .body(body.into())
        .expect("request")
}

async fn upload(
    router: &Router,
    uri: &str,
    file_name: &str,
    content: &[u8],
) -> (StatusCode, Value) {
    let (content_type, body) = multipart(file_name, content);
    send(router, post(uri, &content_type, body)).await
}

async fn json_import(router: &Router, uri: &str, body: &Value) -> (StatusCode, Value) {
    send(router, post(uri, "application/json", body.to_string())).await
}

fn encoded(file_name: &str, content: &[u8]) -> Value {
    json!({ "file_name": file_name, "content": STANDARD.encode(content) })
}

/// Serves the recorded answer on loopback and returns the endpoint to configure.
async fn recorded_dlc_service() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let app = Router::new().route(
        "/service.php",
        axum::routing::get(|| async { DLC_SERVICE_ANSWER }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}/service.php")
}

/// A fresh installation with the DLC import switched on against the recorded service.
async fn dlc_installation(directory: &std::path::Path) -> Router {
    let router = test_router(directory).await;
    let endpoint = recorded_dlc_service().await;
    let (status, mut settings) = get_json(&router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "settings: {settings}");
    settings["admin_login_disabled"] = Value::Bool(true);
    settings["dlc_service_enabled"] = Value::Bool(true);
    settings["dlc_service_endpoint"] = Value::String(endpoint);
    let (status, body) = put_json(&router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "settings: {body}");
    router
}

/// The part of a LinkGrabber answer that describes the file, not the installation it landed in.
fn links(answer: &Value) -> Vec<(String, String, String)> {
    let mut rows: Vec<_> = answer["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|candidate| {
            let url = candidate["url"].as_str().unwrap_or_default();
            // A torrent's candidate points at where this installation stored the file.
            let url = if url.starts_with("file:") {
                "file:"
            } else {
                url
            };
            (
                url.to_owned(),
                candidate["file_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                candidate["size"].to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn package_names(answer: &Value) -> Vec<String> {
    let mut names: Vec<String> = answer["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .filter_map(|package| package["name"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_dlc_arrives_as_json_and_matches_the_upload() {
    let (first, second) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let by_upload = dlc_installation(first.path()).await;
    let by_json = dlc_installation(second.path()).await;

    let (status, uploaded) = upload(&by_upload, "/api/v1/containers/import", "test.dlc", DLC).await;
    assert_eq!(status, StatusCode::CREATED, "{uploaded}");
    let (status, sent) = json_import(
        &by_json,
        "/api/v1/containers/import",
        &encoded("test.dlc", DLC),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");

    assert_eq!(sent["format"], "dlc");
    assert!(
        !links(&sent).is_empty(),
        "the DLC produced no links: {sent}"
    );
    assert_eq!(links(&sent), links(&uploaded));
    assert_eq!(package_names(&sent), package_names(&uploaded));

    // And it is in the LinkGrabber, not only in the answer.
    let (_, packages) = get_json(&by_json, "/api/v1/collector/packages").await;
    assert_eq!(
        packages.as_array().map(Vec::len),
        sent["packages"].as_array().map(Vec::len),
        "{packages}"
    );
}

#[tokio::test]
async fn a_torrent_arrives_as_json_and_matches_the_upload() {
    let (first, second) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let by_upload = test_router(first.path()).await;
    let by_json = test_router(second.path()).await;

    let (status, uploaded) = upload(
        &by_upload,
        "/api/v1/torrents/import",
        "bbb.torrent",
        TORRENT,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{uploaded}");
    let mut body = encoded("bbb.torrent", TORRENT);
    body["priority"] = json!("high");
    let (status, sent) = json_import(&by_json, "/api/v1/torrents/import", &body).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");

    assert_eq!(links(&sent).len(), 1, "{sent}");
    assert_eq!(links(&sent), links(&uploaded));
    assert_eq!(package_names(&sent), package_names(&uploaded));
    assert_eq!(sent["candidates"][0]["priority"], "high", "{sent}");

    let (_, candidates) = get_json(&by_json, "/api/v1/collector/candidates").await;
    assert_eq!(candidates.as_array().map(Vec::len), Some(1), "{candidates}");
}

#[tokio::test]
async fn an_nzb_arrives_as_json_and_matches_the_upload() {
    let (first, second) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let by_upload = test_router(first.path()).await;
    let by_json = test_router(second.path()).await;
    let name = "test_download_100MB{{secret}}.nzb";

    let (status, uploaded) = upload(&by_upload, "/api/v1/nzb/imports", name, NZB).await;
    assert_eq!(status, StatusCode::CREATED, "{uploaded}");
    let (status, sent) = json_import(&by_json, "/api/v1/nzb/imports", &encoded(name, NZB)).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");

    for field in [
        "name",
        "sha256",
        "file_count",
        "segment_count",
        "total_bytes",
        "has_password",
    ] {
        assert_eq!(
            sent[field], uploaded[field],
            "{field}: {sent} vs {uploaded}"
        );
    }
    assert_eq!(sent["sha256"], hex::encode(Sha256::digest(NZB)));
    assert_eq!(
        sent["has_password"], true,
        "the marker in the name was read: {sent}"
    );

    let (_, imports) = get_json(&by_json, "/api/v1/nzb/imports").await;
    assert_eq!(imports.as_array().map(Vec::len), Some(1), "{imports}");
}

#[tokio::test]
async fn content_that_is_not_base64_is_refused_with_its_code_on_every_route() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let body = json!({ "file_name": "x.torrent", "content": "this is not base64!" });
    for uri in [
        "/api/v1/containers/import",
        "/api/v1/dlc/import",
        "/api/v1/torrents/import",
        "/api/v1/nzb/imports",
    ] {
        let (status, answer) = json_import(&router, uri, &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {answer}");
        assert_eq!(answer["code"], "container.base64_invalid", "{uri}");
    }
}

#[tokio::test]
async fn content_that_is_not_a_container_is_refused_with_the_code_the_upload_gets() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let junk = b"this is text, not a container";
    for (uri, file_name, code) in [
        (
            "/api/v1/torrents/import",
            "x.torrent",
            "torrent.file_invalid",
        ),
        ("/api/v1/nzb/imports", "x.nzb", "nzb.parse_failed"),
        (
            "/api/v1/containers/import",
            "x.rsdf",
            "container.file_invalid",
        ),
        (
            "/api/v1/containers/import",
            "x.exe",
            "container.format_unknown",
        ),
    ] {
        let (status, sent) = json_import(&router, uri, &encoded(file_name, junk)).await;
        let (upload_status, uploaded) = upload(&router, uri, file_name, junk).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {file_name}: {sent}");
        assert_eq!(sent["code"], code, "{uri} {file_name}");
        assert_eq!((upload_status, &uploaded["code"]), (status, &sent["code"]));
    }
}

#[tokio::test]
async fn a_body_the_route_does_not_know_is_refused_rather_than_guessed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let body = json!({ "file_name": "x.txt", "contents": "aGVsbG8=" });
    let (status, answer) = json_import(&router, "/api/v1/containers/import", &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "request.body_invalid");
}

/// Base64 of a file one group over 48 MiB: all zeros, so it is valid and only its size is wrong.
fn oversized_body(extra_padding: usize) -> Vec<u8> {
    let mut body = br#"{"file_name":"big.nzb","content":""#.to_vec();
    body.resize(body.len() + 64 * 1024 * 1024 + 4, b'A');
    body.extend_from_slice(br#"","name":""#);
    body.resize(body.len() + extra_padding, b'x');
    body.extend_from_slice(br#""}"#);
    body
}

#[tokio::test]
async fn a_file_over_the_json_limit_gets_a_code_and_is_not_truncated() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, answer) = send(
        &router,
        post("/api/v1/nzb/imports", "application/json", oversized_body(0)),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{answer}");
    assert_eq!(answer["code"], "container.too_large");
    assert_eq!(answer["params"]["max_mib"], "48");
    let (_, imports) = get_json(&router, "/api/v1/nzb/imports").await;
    assert_eq!(
        imports.as_array().map(Vec::len),
        Some(0),
        "nothing was stored: {imports}"
    );
}

/// Over the service-wide limit, with and without a declared length: the first is refused
/// before any route runs, the second while the body is read. Both carry the same code.
#[tokio::test]
async fn a_body_over_the_service_limit_gets_a_code_either_way() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let body = oversized_body(2 * 1024 * 1024);
    assert!(body.len() > 65 * 1024 * 1024);

    let declared = Request::post("/api/v1/containers/import")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body.clone()))
        .expect("request");
    let (status, answer) = send(&router, declared).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{answer}");
    assert_eq!(answer["code"], "request.body_too_large");
    assert_eq!(answer["params"]["max_mib"], "65");

    let streamed = post("/api/v1/containers/import", "application/json", body);
    let (status, answer) = send(&router, streamed).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{answer}");
    assert_eq!(answer["code"], "request.body_too_large");
}

/// The JSON body costs `api:intake`, like the upload on the same route — refused without it,
/// accepted with it and nothing else.
#[tokio::test]
async fn the_json_body_costs_the_intake_permission() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let mut bearers = Vec::new();
    for (label, scopes) in [
        (
            "everything-but-intake",
            vec![
                "api:read",
                "api:queue",
                "api:config",
                "api:secrets",
                "api:admin",
            ],
        ),
        ("intake-only", vec!["api:intake"]),
    ] {
        let bearer = format!("container-json-{label}");
        harness
            .database
            .create_capture_token(
                rd_core::CaptureTokenId::new(),
                label.to_owned(),
                hex::encode(Sha256::digest(bearer.as_bytes())),
                scopes.into_iter().map(str::to_owned).collect(),
            )
            .await
            .expect("token");
        bearers.push(bearer);
    }
    let list = encoded("links.txt", b"https://example.invalid/a.bin\n").to_string();
    let request = |bearer: &str| {
        Request::post("/api/v1/containers/import")
            .header(header::HOST, "127.0.0.1:8710")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
            .body(Body::from(list.clone()))
            .expect("request")
    };

    let (status, answer) = send(&harness.router, request(&bearers[0])).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["code"], "auth.scope_insufficient");
    assert_eq!(answer["params"]["scope"], "api:intake");

    let (status, answer) = send(&harness.router, request(&bearers[1])).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
}

#[tokio::test]
async fn a_remote_job_takes_a_container_through_the_same_decoder() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let account = "00000000-0000-7000-8000-000000000000";
    let uri = format!("/api/v1/accounts/{account}/remote-jobs");

    let (status, answer) = json_import(&router, &uri, &json!({ "container": "not base64!" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "container.base64_invalid");

    // 16 MiB and one byte, which the import routes would take and a remote job does not.
    let over = STANDARD.encode(vec![0_u8; 16 * 1024 * 1024 + 1]);
    let (status, answer) = json_import(&router, &uri, &json!({ "container": over })).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{answer}");
    assert_eq!(answer["code"], "container.too_large");
    assert_eq!(answer["params"]["max_mib"], "16");

    let both = json!({ "container": STANDARD.encode(TORRENT), "magnet": "magnet:?xt=urn:btih:0" });
    let (status, answer) = json_import(&router, &uri, &both).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "remote_job.source_invalid");

    // A well-formed container gets as far as the account, which this installation lacks.
    let (status, answer) = json_import(
        &router,
        &uri,
        &json!({ "container": STANDARD.encode(TORRENT) }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "remote_job.no_account");
}

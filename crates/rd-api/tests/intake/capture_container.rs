//! Link containers through `POST /api/v1/capture/file` (RD-1260-09): the capture agent hands over
//! a container it was opened with, dropped on or sent to, and the extension one it intercepted --
//! the LinkGrabber's container import behind the capture token, one test per format the agent
//! registers. The name decides, before the content is looked at.
//!
//! A DLC and a CCF can only be opened by an online service, which ships switched off; that they
//! reach it is shown by its refusal. The other three are read here, without the network.

use crate::common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{API_BEARER, CAPTURE_BEARER, get_json, send, test_router};

const ROUTE: &str = "/api/v1/capture/file";

/// A one-file NZB, as an `.rdlinks` export carries it whole.
const NZB: &str = r#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="tester" subject="show.s01e01.mkv">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="42" number="1">show-part1@example.test</segment></segments>
  </file>
</nzb>"#;

/// A multipart upload of one file, as the agent and the extension send it.
async fn hand_over(
    router: &Router,
    bearer: Option<&str>,
    file_name: &str,
    content: &[u8],
) -> (StatusCode, serde_json::Value) {
    let boundary = "rdcontainerboundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut request = Request::post(ROUTE)
        .header(header::HOST, "127.0.0.1:8710")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        );
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    send(router, request.body(Body::from(body)).expect("request")).await
}

/// The addresses the LinkGrabber holds now.
async fn linkgrabber_urls(router: &Router) -> Vec<String> {
    let (status, candidates) = get_json(router, "/api/v1/collector/candidates").await;
    assert_eq!(status, StatusCode::OK, "{candidates}");
    candidates
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|candidate| candidate["url"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_crawljob_reaches_the_linkgrabber() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = hand_over(
        &router,
        Some(CAPTURE_BEARER),
        "holiday.crawljob",
        b"text=https://files.example/holiday.part1.rar https://files.example/holiday.part2.rar\npackageName=Holiday\nautoStart=TRUE\n",
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "container", "{body}");
    assert_eq!(body["container"]["format"], "crawljob", "{body}");
    let urls = linkgrabber_urls(&router).await;
    assert!(
        urls.contains(&"https://files.example/holiday.part1.rar".to_owned()),
        "{urls:?}"
    );
    assert_eq!(urls.len(), 2, "{urls:?}");
}

#[tokio::test]
async fn an_rdlinks_file_reaches_the_linkgrabber() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let document = serde_json::json!({ "format": "rdownloader-links/1", "packages": [
        { "name": "Holiday", "links": [{ "url": "https://files.example/holiday.rar" }] }
    ]});

    let (status, body) = hand_over(
        &router,
        Some(CAPTURE_BEARER),
        "holiday.rdlinks",
        document.to_string().as_bytes(),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "container", "{body}");
    assert_eq!(body["container"]["format"], "rdlinks", "{body}");
    assert_eq!(
        linkgrabber_urls(&router).await,
        vec!["https://files.example/holiday.rar".to_owned()]
    );
}

/// An `.rdlinks` is JSON that may carry whole NZB documents, whose `<nzb` the content sniff would
/// find: it goes to the container import by its name, and its NZB comes out as an import of it.
#[tokio::test]
async fn an_rdlinks_carrying_an_nzb_is_a_container_not_an_nzb() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let document = serde_json::json!({ "format": "rdownloader-links/1", "packages": [
        { "name": "Show", "links": [], "nzbs": [{ "name": "Show.S01E01", "content": NZB }] }
    ]});

    let (status, body) = hand_over(
        &router,
        Some(CAPTURE_BEARER),
        "show.rdlinks",
        document.to_string().as_bytes(),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "container", "{body}");
    assert_eq!(body["container"]["format"], "rdlinks", "{body}");
    assert_eq!(
        body["nzb_imports"].as_array().map(Vec::len),
        Some(0),
        "not taken for an NZB itself: {body}"
    );
    assert_eq!(
        body["container"]["nzb_imports"][0]["name"], "Show.S01E01",
        "{body}"
    );
}

#[tokio::test]
async fn an_rsdf_is_read_by_the_rsdf_reader() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    // Valid hex and valid base64, but not this format's encryption: the RSDF reader refuses it,
    // which no other format's reader would have been asked to.
    let content = hex::encode("EREREREREREREREREREREQ==");
    let (status, body) = hand_over(
        &router,
        Some(CAPTURE_BEARER),
        "broken.rsdf",
        content.as_bytes(),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "container.file_invalid", "{body}");
}

#[tokio::test]
async fn a_dlc_and_a_ccf_go_to_the_decryption_service() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    for name in ["release.dlc", "release.ccf"] {
        let (status, body) =
            hand_over(&router, Some(CAPTURE_BEARER), name, b"not decrypted here").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert_eq!(body["code"], "dlc.service_disabled", "{name}: {body}");
    }
}

/// A name that is no container, and a text list (a container of the import dialog, but too
/// general a name here), leave the content to decide -- and a list of links is not a file this
/// route takes.
#[tokio::test]
async fn a_file_that_names_no_container_is_refused_with_its_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    for name in ["notes.md", "links.txt"] {
        let (status, body) = hand_over(
            &router,
            Some(CAPTURE_BEARER),
            name,
            b"https://files.example/a.bin\n",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert_eq!(body["code"], "capture.file_unsupported", "{name}: {body}");
    }
    assert!(linkgrabber_urls(&router).await.is_empty());
}

/// The capture surface: no token, or a token of the API, gets nothing in.
#[tokio::test]
async fn only_a_capture_token_hands_a_container_over() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let crawljob = b"text=https://files.example/a.bin\n";

    let (status, body) = hand_over(&router, None, "a.crawljob", crawljob).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, body) = hand_over(&router, Some(API_BEARER), "a.crawljob", crawljob).await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{status}: {body}"
    );
    assert!(linkgrabber_urls(&router).await.is_empty());
}

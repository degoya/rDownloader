//! RD-080-06: direct HLS and DASH manifests through the LinkGrabber.
//!
//! The parsing itself is unit-tested in `rd-media`; what is tested here is the routing that
//! parsing decides — a manifest reaches the media provider at all, a DRM-protected one is
//! refused instead of queued, and a live one is handed to the recorder rather than to the
//! file downloader, which would produce a job that can never finish.

mod common;

use axum::{Router, routing::get};
use common::{get_json, post_json, test_router, wait_for_candidates_ready};
use serde_json::json;

const MASTER: &str = "#EXTM3U\n\
    #EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=1280x720\n\
    720p/index.m3u8\n\
    #EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x1080\n\
    1080p/index.m3u8\n";

const LIVE: &str = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:42\n#EXTINF:6.0,\nseg42.ts\n";

const VOD: &str = "#EXTM3U\n#EXTINF:6.0,\nseg1.ts\n#EXT-X-ENDLIST\n";

const DRM: &str = "#EXTM3U\n\
    #EXT-X-KEY:METHOD=SAMPLE-AES,KEYFORMAT=\"com.apple.streamingkeydelivery\",URI=\"skd://x\"\n\
    #EXTINF:6.0,\nseg1.ts\n#EXT-X-ENDLIST\n";

/// Serves the four fixtures, plus one manifest with no telling extension.
async fn fixture_server() -> String {
    let app = Router::new()
        .route("/master.m3u8", get(|| async { MASTER }))
        .route("/live.m3u8", get(|| async { LIVE }))
        .route("/vod.m3u8", get(|| async { VOD }))
        .route("/drm.m3u8", get(|| async { DRM }))
        // A signed CDN address: no extension, and served as text/plain.
        .route(
            "/stream/token/abc",
            get(|| async { ([("content-type", "text/plain")], LIVE) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}")
}

/// Adds one link and returns the candidate the check settled on.
async fn intake(router: &Router, url: &str) -> serde_json::Value {
    let (status, _) = post_json(
        router,
        "/api/v1/collector/batches",
        json!({
            "text": url,
            "source": "api",
            "source_label": null,
            "package_name": null,
            "password": null
        }),
    )
    .await;
    assert!(status.is_success(), "intake failed: {status}");
    wait_for_candidates_ready(router).await;
    let (_, candidates) = get_json(router, "/api/v1/collector/candidates").await;
    candidates
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["url"].as_str() == Some(url))
        .cloned()
        .unwrap_or_else(|| panic!("no candidate for {url}"))
}

#[tokio::test]
async fn a_manifest_url_reaches_the_media_provider_rather_than_the_http_engine() {
    // Without this the playlist *text* was downloaded and presented as the video.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let server = fixture_server().await;

    let candidate = intake(&router, &format!("{server}/master.m3u8")).await;
    assert_ne!(
        candidate["provider"].as_str(),
        Some("direct_http"),
        "manifest was treated as a plain file: {candidate}"
    );
}

#[tokio::test]
async fn a_live_manifest_is_routed_to_the_recorder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let server = fixture_server().await;

    let candidate = intake(&router, &format!("{server}/live.m3u8")).await;
    assert_eq!(
        candidate["provider"].as_str(),
        Some("record"),
        "a playlist with no #EXT-X-ENDLIST must go to the recorder: {candidate}"
    );
}

#[tokio::test]
async fn a_vod_manifest_stays_on_the_download_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let server = fixture_server().await;

    let candidate = intake(&router, &format!("{server}/vod.m3u8")).await;
    assert_eq!(candidate["provider"].as_str(), Some("media"));
}

#[tokio::test]
async fn a_drm_protected_manifest_is_refused_instead_of_queued() {
    // DRM is a stated non-goal, so it has to fail here rather than obscurely inside ffmpeg.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let server = fixture_server().await;

    let candidate = intake(&router, &format!("{server}/drm.m3u8")).await;
    let error = candidate["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("DRM"),
        "expected a DRM refusal, got {candidate}"
    );
    assert_ne!(candidate["state"].as_str(), Some("online"));
}

#[tokio::test]
async fn a_manifest_without_an_extension_is_still_recognised() {
    // The address says nothing and the content type says text/plain; only the body knows.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let server = fixture_server().await;

    let candidate = intake(&router, &format!("{server}/stream/token/abc")).await;
    assert_eq!(
        candidate["provider"].as_str(),
        Some("record"),
        "an extension-less live manifest was not recognised: {candidate}"
    );
}

#[tokio::test]
async fn an_ordinary_file_is_not_mistaken_for_a_manifest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let app = Router::new().route(
        "/clip.mp4",
        get(|| async {
            (
                [("content-type", "video/mp4")],
                "\u{0}\u{0}\u{0}\u{18}ftypmp42",
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let candidate = intake(&router, &format!("http://{address}/clip.mp4")).await;
    assert_eq!(candidate["provider"].as_str(), Some("direct_http"));
    assert_eq!(candidate["error"].as_str(), None, "{candidate}");
}

#[tokio::test]
async fn a_media_link_without_a_selection_is_refused_instead_of_queued() {
    // RD-120-50: such a link used to become a queue row whose first step was
    // `media.selection_missing` — a download that could only fail, reported far from the
    // reason. The test service has no yt-dlp, so the manifest's probe yields no variants.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let server = fixture_server().await;

    let candidate = intake(&router, &format!("{server}/vod.m3u8")).await;
    assert_eq!(candidate["provider"].as_str(), Some("media"), "{candidate}");
    assert!(
        candidate["media"].is_null(),
        "the probe must have produced no selection here: {candidate}"
    );
    let id = candidate["id"].as_str().expect("candidate id");
    let (status, body) = post_json(
        &router,
        &format!("/api/v1/collector/candidates/{id}/enqueue"),
        json!({}),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert_eq!(body["code"], "collector.media_selection_missing", "{body}");
    assert_eq!(body["params"]["candidate_id"], id, "{body}");
    let (_, downloads) = get_json(&router, "/api/v1/downloads").await;
    assert_eq!(
        downloads.as_array().map(Vec::len),
        Some(0),
        "nothing may be queued: {downloads}"
    );
}

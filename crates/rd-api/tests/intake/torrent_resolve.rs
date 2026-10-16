//! *Resolve* on a `.torrent` link reads the file on the link's own network (RD-1320-05).
//!
//! It handed librqbit the address, which fetched it with a client of its own — without the
//! authentication profile, the proxy or the custom CA. The tracker below answers only a request
//! that carries the profile's token; the profile is created after the link's online check, so
//! the check leaves the link without metadata and *Resolve* has to fetch it.

use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use serde_json::json;

use crate::common::{get_json, post_json, test_harness, wait_for_candidates_ready};

const TOKEN: &str = "torrent-token-41d2";

fn bstr(value: &str) -> Vec<u8> {
    let mut out = format!("{}:", value.len()).into_bytes();
    out.extend_from_slice(value.as_bytes());
    out
}

/// A one-byte single-file torrent named `resolved.bin`.
fn torrent() -> Vec<u8> {
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("announce"));
    bytes.extend(bstr("http://tracker.test/announce"));
    bytes.extend(bstr("info"));
    bytes.push(b'd');
    bytes.extend(bstr("length"));
    bytes.extend_from_slice(b"i1e");
    bytes.extend(bstr("name"));
    bytes.extend(bstr("resolved.bin"));
    bytes.extend(bstr("piece length"));
    bytes.extend_from_slice(b"i16384e");
    bytes.extend(bstr("pieces"));
    bytes.extend_from_slice(b"20:");
    bytes.extend_from_slice(&[7_u8; 20]);
    bytes.extend_from_slice(b"ee");
    bytes
}

#[tokio::test]
async fn resolving_a_torrent_link_fetches_it_with_the_links_profile() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let log = Arc::clone(&seen);
    let tracker = rd_testkit::serve(axum::Router::new().route(
        "/t/release.torrent",
        axum::routing::get(move |headers: HeaderMap| {
            let log = Arc::clone(&log);
            async move {
                let authorization = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                rd_testkit::lock(&log).push(authorization.clone());
                if authorization.as_deref() == Some(&format!("Bearer {TOKEN}")) {
                    rd_testkit::respond(
                        StatusCode::OK,
                        &[("content-type", "application/x-bittorrent".to_owned())],
                        torrent(),
                    )
                } else {
                    rd_testkit::respond(StatusCode::FORBIDDEN, &[], "signed-in members only")
                }
            }
        }),
    ))
    .await;

    // The person's own paste: a program's link keeps to the address rule, which refuses this
    // machine, where the tracker listens (RD-1190-18).
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/batches",
        json!({ "text": tracker.url("/t/release.torrent"), "source": "manual", "source_label": null, "package_name": null, "password": null }),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    wait_for_candidates_ready(&harness.router).await;
    let (_, candidates) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    let candidate = candidates[0]["id"].as_str().expect("candidate").to_owned();

    let secret_ref = harness
        .secrets
        .put_string(TOKEN.to_owned())
        .await
        .expect("vault");
    harness
        .database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: "tracker".to_owned(),
            scope: rd_core::AuthScope::parse("127.0.0.1", false).expect("scope"),
            method: rd_core::AuthMethod::Bearer,
            origin: rd_core::AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: Some(secret_ref),
            certificate_ref: None,
        })
        .await
        .expect("profile");
    rd_testkit::lock(&seen).clear();

    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/collector/candidates/{candidate}/torrent/resolve"),
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["metadata_state"], "ready", "{body}");
    assert_eq!(body["name"], "resolved.bin", "{body}");
    let expected = format!("Bearer {TOKEN}");
    assert_eq!(
        rd_testkit::lock(&seen).clone(),
        [Some(expected)],
        "fetched once, with the profile"
    );
}

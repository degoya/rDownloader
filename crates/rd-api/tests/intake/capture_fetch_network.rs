//! The hand-over fetch of `capture/file` goes out on the service's own network (RD-1320-06).
//!
//! Before, it built a client of its own beside the global proxy: a person behind a proxy
//! showed the site their own address, and a company proxy with its own CA failed. Here the
//! global proxy is a local listener that answers every request itself, and the target's name
//! does not resolve, so a hand-over that arrives went through it.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::http::StatusCode;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::common::{self, CAPTURE_BEARER, post_with_bearer, test_harness};

const ROUTE: &str = "/api/v1/capture/file";

const NZB: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="tester" subject="proxied.bin">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="42" number="1">proxied@example</segment></segments>
  </file>
</nzb>"#;

/// The request line and `Cookie` header of every request the proxy saw.
type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A plain HTTP proxy that answers every request with the NZB and records its request line and
/// `Cookie` header.
async fn proxy() -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let seen = Log::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => raw.extend_from_slice(&buffer[..read]),
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                let line = text.lines().next().unwrap_or_default().to_owned();
                let cookie = text.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("cookie")
                        .then(|| value.trim().to_owned())
                });
                rd_testkit::lock(&log).push((line, cookie));
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/x-nzb\r\n\
                     content-disposition: attachment; filename=\"Proxied.nzb\"\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{NZB}",
                    NZB.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (format!("http://{address}"), seen)
}

/// Makes the proxy profile at `endpoint` the global one.
async fn global_proxy(
    harness: &common::Harness,
    endpoint: &str,
    username: Option<&str>,
    secret_ref: Option<&str>,
) {
    let profile = harness
        .database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "global".to_owned(),
            kind: rd_core::ProxyKind::Http,
            endpoint: endpoint.parse().expect("endpoint"),
            username: username.map(str::to_owned),
            secret_ref: secret_ref.map(str::to_owned),
            // The fixture plays the proxy and resolves the names itself (RD-1320-07).
            resolves_names: true,
        })
        .await
        .expect("proxy profile");
    harness
        .scheduler
        .update_runtime_settings(rd_scheduler::RuntimeSettings {
            global_proxy_profile_id: Some(profile.id),
            ..rd_scheduler::RuntimeSettings::default()
        })
        .await
        .expect("global proxy");
}

/// The acceptance case: with a global proxy the hand-over goes through it, cookies and all.
#[tokio::test]
async fn the_hand_over_is_fetched_through_the_global_proxy() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (endpoint, seen) = proxy().await;
    global_proxy(&harness, &endpoint, None, None).await;

    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "url": "http://indexer.test/getnzb/abc", "cookies": "uid=7" }),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["nzb_imports"][0]["name"], "Proxied.nzb", "{body}");
    assert_eq!(
        rd_testkit::lock(&seen).clone(),
        [(
            "GET http://indexer.test/getnzb/abc HTTP/1.1".to_owned(),
            Some("uid=7".to_owned())
        )]
    );
}

/// A global proxy whose password cannot be read is no reason to go direct: the hand-over fails,
/// and the server — which a direct fetch would reach — is never asked.
#[tokio::test]
async fn an_unusable_proxy_fails_the_hand_over_instead_of_going_direct() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(
        directory.path(),
        common::Options::default().local_capture_fetches(),
    )
    .await;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let server = rd_testkit::serve(axum::Router::new().fallback(move || {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            NZB
        }
    }))
    .await;
    let (endpoint, seen) = proxy().await;
    // A vault reference nothing was ever stored under.
    let missing = "00000000-0000-4000-8000-000000000000";
    global_proxy(&harness, &endpoint, Some("user"), Some(missing)).await;

    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "url": server.url("/getnzb/abc") }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "proxy.check_unavailable", "{body}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "fetched directly");
    assert!(rd_testkit::lock(&seen).is_empty());
}

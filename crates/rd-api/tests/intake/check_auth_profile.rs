//! The online check goes out with the candidate's own authentication profile (RD-1320-04).
//!
//! Before, every check asked for the automatic profile of the address, whatever the person had
//! chosen for the link: a pinned profile was passed over, and "no profile" still sent the one
//! that matched. The transfer honoured the choice, so the check and the download could see two
//! different servers. And the WebDAV `PROPFIND` dropped a Basic or Bearer profile's
//! `Authorization` altogether, though it promised to authenticate exactly the way the download
//! does.
//!
//! One local server records the `Authorization` of every request; the profiles are told apart
//! by their tokens.

use std::sync::{Arc, Mutex};

use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use rd_core::{AuthMethod, AuthOrigin, AuthProfileId, AuthScope};
use serde_json::json;

use crate::common::{
    self, eventually, get_json, post_json, put_json, test_harness, wait_for_candidates_ready,
};

/// What the server saw of one request.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

const LISTING: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
  </D:response>
  <D:response><D:href>/dav/file.bin</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/><D:getcontentlength>4096</D:getcontentlength>
    </D:prop></D:propstat>
  </D:response>
</D:multistatus>"#;

/// `/file` is a named file; a `PROPFIND` anywhere answers with a one-file listing.
async fn serve(log: Log) -> rd_testkit::TestServer {
    let app = axum::Router::new().fallback(move |request: Request| {
        let log = Arc::clone(&log);
        async move {
            rd_testkit::lock(&log).push(Seen {
                method: request.method().to_string(),
                path: request.uri().path().to_owned(),
                authorization: request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
            });
            answer(request.method())
        }
    });
    rd_testkit::serve(app).await
}

fn answer(method: &Method) -> Response {
    if method.as_str() == "PROPFIND" {
        return rd_testkit::respond(
            StatusCode::MULTI_STATUS,
            &[
                ("content-type", "application/xml".to_owned()),
                ("accept-ranges", "bytes".to_owned()),
            ],
            LISTING,
        );
    }
    rd_testkit::respond(
        StatusCode::OK,
        &[
            ("content-type", "application/octet-stream".to_owned()),
            (
                "content-disposition",
                "attachment; filename=\"named.bin\"".to_owned(),
            ),
        ],
        rd_testkit::payload(4_096),
    )
}

/// An enabled profile for `scope` whose secret is `secret`.
async fn profile(
    harness: &common::Harness,
    scope: &str,
    method: AuthMethod,
    username: Option<&str>,
    secret: &str,
) -> AuthProfileId {
    let secret_ref = harness
        .secrets
        .put_string(secret.to_owned())
        .await
        .expect("vault");
    harness
        .database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: format!("{scope} {secret}"),
            scope: AuthScope::parse(scope, false).expect("scope"),
            method,
            origin: AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: username.map(str::to_owned),
            secret_ref: Some(secret_ref),
            certificate_ref: None,
        })
        .await
        .expect("profile")
        .id
}

/// Adds `url` the person's own way and returns its candidate id once its first check is done.
///
/// `manual`, as the person's paste: a link a program sent (`api`) keeps to the address rule and
/// is never requested on this machine (RD-1190-18), where the test server listens.
async fn intake(router: &axum::Router, url: &str) -> String {
    let (status, body) = post_json(
        router,
        "/api/v1/collector/batches",
        json!({ "text": url, "source": "manual", "source_label": null, "package_name": null, "password": null }),
    )
    .await;
    assert!(status.is_success(), "intake failed: {status} {body}");
    wait_for_candidates_ready(router).await;
    let (_, candidates) = get_json(router, "/api/v1/collector/candidates").await;
    candidates[0]["id"]
        .as_str()
        .expect("candidate id")
        .to_owned()
}

/// Sets the candidate's profile choice, checks it again and returns what that check sent.
async fn recheck_with(
    router: &axum::Router,
    log: &Log,
    candidate: &str,
    choice: serde_json::Value,
) -> Vec<Seen> {
    let (status, body) = put_json(
        router,
        &format!("/api/v1/collector/candidates/{candidate}/auth-profile"),
        choice,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    rd_testkit::lock(log).clear();
    let (status, body) = post_json(
        router,
        "/api/v1/collector/candidates/check",
        json!({ "ids": [candidate] }),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    eventually(
        std::time::Duration::from_secs(5),
        "the check never reached the server",
        move || async move { (!rd_testkit::lock(log).is_empty()).then_some(()) },
    )
    .await;
    wait_for_candidates_ready(router).await;
    rd_testkit::lock(log).clone()
}

/// Every request in `seen` carried exactly `expected`.
fn all_carry(seen: &[Seen], expected: Option<&str>) {
    assert!(!seen.is_empty(), "the server was never asked");
    for request in seen {
        assert_eq!(request.authorization.as_deref(), expected, "{request:?}");
    }
}

/// The acceptance case: the automatic choice sends the most specific matching profile, a pinned
/// one is sent in its place, and "no profile" sends nothing although a profile matches.
#[tokio::test]
async fn a_check_sends_the_automatic_the_pinned_or_no_profile_as_the_link_says() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let log = Log::default();
    let server = serve(Arc::clone(&log)).await;
    // The automatic choice is the one with the path prefix; the other covers the host only.
    profile(
        &harness,
        "127.0.0.1/file",
        AuthMethod::Bearer,
        None,
        "token-auto",
    )
    .await;
    let pinned = profile(
        &harness,
        "127.0.0.1",
        AuthMethod::Bearer,
        None,
        "token-pinned",
    )
    .await;

    let candidate = intake(&harness.router, &server.url("/file")).await;
    all_carry(&rd_testkit::lock(&log).clone(), Some("Bearer token-auto"));

    let seen = recheck_with(
        &harness.router,
        &log,
        &candidate,
        json!({ "mode": "pinned", "profile_id": pinned.to_string() }),
    )
    .await;
    all_carry(&seen, Some("Bearer token-pinned"));

    let seen = recheck_with(&harness.router, &log, &candidate, json!({ "mode": "none" })).await;
    all_carry(&seen, None);

    let seen = recheck_with(&harness.router, &log, &candidate, json!({ "mode": "auto" })).await;
    all_carry(&seen, Some("Bearer token-auto"));
}

/// A WebDAV check sends the `Authorization` of a Basic profile it picks itself, and of a Bearer
/// profile the link is pinned to.
#[tokio::test]
async fn a_webdav_check_sends_the_basic_or_bearer_profiles_authorization() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let log = Log::default();
    let server = serve(Arc::clone(&log)).await;
    profile(
        &harness,
        "127.0.0.1",
        AuthMethod::Basic,
        Some("dav"),
        "secret",
    )
    .await;

    let address = server.addr();
    let candidate = intake(&harness.router, &format!("webdav://{address}/dav/")).await;
    let seen = rd_testkit::lock(&log).clone();
    let propfind: Vec<Seen> = seen
        .into_iter()
        .filter(|request| request.method == "PROPFIND")
        .collect();
    // `dav:secret` in Base64.
    all_carry(&propfind, Some("Basic ZGF2OnNlY3JldA=="));
    assert!(propfind.iter().all(|request| request.path == "/dav/"));

    // Created only now, so the automatic choice above had one profile to pick; its own path
    // prefix, since one scope holds one profile (`auth_profiles_scope_idx`).
    let bearer = profile(
        &harness,
        "127.0.0.1/dav",
        AuthMethod::Bearer,
        None,
        "dav-token",
    )
    .await;
    let seen = recheck_with(
        &harness.router,
        &log,
        &candidate,
        json!({ "mode": "pinned", "profile_id": bearer.to_string() }),
    )
    .await;
    all_carry(&seen, Some("Bearer dav-token"));
}

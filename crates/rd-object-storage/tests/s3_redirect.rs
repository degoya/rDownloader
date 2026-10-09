//! An endpoint that answers with a redirect (RD-1200-06).
//!
//! The endpoint keeps to the address rule (RD-1190-18), but a redirect to a literal address
//! would never ask the resolver that holds it there. So the store's client follows no redirect
//! at all: the endpoint below sends every request on to a second listener on `127.0.0.1`, and
//! neither the connection test nor a download reaches it — both fail with
//! `object_storage.redirect_refused`.

mod support;

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, Uri},
    response::Response,
};
use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider};
use rd_db::NewObjectStorageProfile;
use rd_object_storage::error::REDIRECT_REFUSED;
use rd_scheduler::RunOutcome;

use support::{Harness, lock, respond, serve};

const BUCKET: &str = "redirect-bucket";

/// The paths that reached the redirect's target.
type Seen = Arc<Mutex<Vec<String>>>;

/// The target: records every request and answers it as an empty bucket would, so a followed
/// redirect would make the test pass where it must fail.
async fn target(State(seen): State<Seen>, uri: Uri) -> Response {
    lock(&seen).push(uri.to_string());
    respond(
        StatusCode::OK,
        &[("content-type", "application/xml".to_owned())],
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name>\
             <Prefix></Prefix><KeyCount>0</KeyCount><MaxKeys>1000</MaxKeys>\
             <IsTruncated>false</IsTruncated></ListBucketResult>"
        ),
    )
}

/// The endpoint: every request is redirected with `status` to the same path on `to`.
async fn redirecting(status: StatusCode, to: SocketAddr) -> SocketAddr {
    let app = Router::new().fallback(move |uri: Uri| async move {
        respond(status, &[("location", format!("http://{to}{uri}"))], "")
    });
    serve(app).await
}

async fn harness(endpoint: SocketAddr) -> Harness {
    Harness::start(
        NewObjectStorageProfile {
            name: "redirect".to_owned(),
            provider: ObjectStorageProvider::S3,
            endpoint: Some(format!("http://{endpoint}")),
            region: Some("us-east-1".to_owned()),
            bucket: Some(BUCKET.to_owned()),
            addressing: ObjectAddressing::Path,
            credential_source: ObjectCredentialSource::Static,
            access_key_id: Some("AKIDEXAMPLE".to_owned()),
            account: None,
            ambient_custom_endpoint: false,
            secret_ref: None,
            session_token_ref: None,
            checksums: false,
            enabled: true,
        },
        Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"),
        "s3",
        BUCKET,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_is_never_followed_by_the_connection_test_or_a_download() {
    let seen = Seen::default();
    let to = serve(Router::new().fallback(target).with_state(seen.clone())).await;
    for status in [
        StatusCode::MOVED_PERMANENTLY,
        StatusCode::TEMPORARY_REDIRECT,
    ] {
        let harness = harness(redirecting(status, to).await).await;
        let failure = harness
            .service
            .test_profile(&harness.profile)
            .await
            .expect("test")
            .expect("a redirect must fail the test");
        assert_eq!(failure.code.as_deref(), Some(REDIRECT_REFUSED), "{status}");

        let (file, package) = harness.queue("movie.mkv", "movie.mkv").await;
        match harness.run(&file, &package).await {
            RunOutcome::Failed(failure) => {
                assert_eq!(failure.code.as_deref(), Some(REDIRECT_REFUSED), "{status}");
            }
            other => panic!("a redirected download must fail, got {other:?}"),
        }
    }
    assert!(
        lock(&seen).is_empty(),
        "no request may reach the redirect's target: {:?}",
        lock(&seen)
    );
}

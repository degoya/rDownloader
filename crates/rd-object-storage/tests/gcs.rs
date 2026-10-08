//! The Google Cloud Storage wire format against a local fixture endpoint (RD-150-05).
//!
//! The fixture answers the XML API the way `storage.googleapis.com` does: objects under
//! `/<bucket>/<percent-encoded name>` with `x-goog-generation`, a read pinned to one
//! generation by `?generation=`, the `list-type=2` listing and the multipart calls. It records
//! the `Authorization` header and the generations asked for.
//!
//! A service account key needs Google's token endpoint, which no test reaches; the key's
//! shape is checked offline, the signed exchange by the ignored live test at the end.

#![cfg(feature = "gcs")]

mod support;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
};
use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider};
use rd_db::NewObjectStorageProfile;
use rd_extract::UploadReport;
use rd_object_storage::error::{AUTH_FAILED, OBJECT_CHANGED, RATE_LIMITED};
use rd_scheduler::RunOutcome;
use support::{Harness, decode, lock, payload, respond};

const BUCKET: &str = "media_bucket";
const LAST_MODIFIED: &str = "Sun, 27 Sep 2026 10:00:00 GMT";

/// Body and generation by object name.
type Objects = BTreeMap<String, (Vec<u8>, u64)>;
/// Open multipart uploads by id: the parts received so far, by part number.
type Uploads = BTreeMap<String, BTreeMap<u32, Vec<u8>>>;

#[derive(Clone, Default)]
struct Fixture {
    /// Body and generation by object name.
    objects: Arc<Mutex<Objects>>,
    uploads: Arc<Mutex<Uploads>>,
    next: Arc<Mutex<u64>>,
    authorizations: Arc<Mutex<Vec<String>>>,
    generations: Arc<Mutex<Vec<String>>>,
    ranges: Arc<Mutex<Vec<String>>>,
    /// Answers every request with this status and body.
    refuse_with: Arc<Mutex<Option<(StatusCode, &'static str)>>>,
}

impl Fixture {
    fn put(&self, name: &str, body: Vec<u8>) -> u64 {
        let mut next = lock(&self.next);
        *next += 1;
        lock(&self.objects).insert(name.to_owned(), (body, *next));
        *next
    }

    fn object(&self, name: &str) -> Option<Vec<u8>> {
        lock(&self.objects).get(name).map(|(body, _)| body.clone())
    }
}

fn xml(status: StatusCode, body: String) -> Response {
    respond(
        status,
        &[("content-type", "application/xml".to_owned())],
        body,
    )
}

fn error(status: StatusCode, code: &str) -> Response {
    xml(status, format!("<Error><Code>{code}</Code></Error>"))
}

/// A generation's validators: GCS derives the ETag from the content, but a fixture that
/// changes it with the generation shows the same thing.
fn validators(generation: u64) -> Vec<(&'static str, String)> {
    vec![
        ("etag", format!("\"g{generation}\"")),
        ("x-goog-generation", generation.to_string()),
        ("x-goog-metageneration", "1".to_owned()),
        ("last-modified", LAST_MODIFIED.to_owned()),
    ]
}

async fn handle(
    State(fixture): State<Fixture>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(value) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        lock(&fixture.authorizations).push(value.to_owned());
    }
    if let Some((status, text)) = *lock(&fixture.refuse_with) {
        return respond(status, &[], text);
    }
    let path = uri.path().trim_start_matches('/');
    let (bucket, name) = path.split_once('/').unwrap_or((path, ""));
    if decode(bucket) != BUCKET {
        return error(StatusCode::NOT_FOUND, "NoSuchBucket");
    }
    let name = decode(name);
    let query: BTreeMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    match method {
        Method::GET if name.is_empty() => list(&fixture, &query),
        Method::HEAD | Method::GET => read(&fixture, &name, &query, &headers),
        Method::PUT if query.contains_key("uploadId") => {
            let number: u32 = query
                .get("partNumber")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let mut uploads = lock(&fixture.uploads);
            let Some(upload) = uploads.get_mut(&query["uploadId"]) else {
                return error(StatusCode::NOT_FOUND, "NoSuchUpload");
            };
            upload.insert(number, body.to_vec());
            respond(
                StatusCode::OK,
                &[("etag", format!("\"part-{number}\""))],
                Body::empty(),
            )
        }
        Method::PUT => {
            let generation = fixture.put(&name, body.to_vec());
            respond(StatusCode::OK, &validators(generation), Body::empty())
        }
        Method::POST if query.contains_key("uploads") => {
            let id = format!("upload-{}", lock(&fixture.uploads).len() + 1);
            lock(&fixture.uploads).insert(id.clone(), BTreeMap::new());
            xml(
                StatusCode::OK,
                format!(
                    "<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{name}</Key>\
                     <UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
                ),
            )
        }
        Method::POST if query.contains_key("uploadId") => {
            let Some(parts) = lock(&fixture.uploads).remove(&query["uploadId"]) else {
                return error(StatusCode::NOT_FOUND, "NoSuchUpload");
            };
            let generation = fixture.put(&name, parts.into_values().flatten().collect());
            respond(
                StatusCode::OK,
                &[
                    ("content-type", "application/xml".to_owned()),
                    ("x-goog-generation", generation.to_string()),
                ],
                format!(
                    "<CompleteMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{name}</Key>\
                     <ETag>\"g{generation}\"</ETag></CompleteMultipartUploadResult>"
                ),
            )
        }
        Method::DELETE if query.contains_key("uploadId") => {
            lock(&fixture.uploads).remove(&query["uploadId"]);
            respond(StatusCode::NO_CONTENT, &[], Body::empty())
        }
        _ => error(StatusCode::BAD_REQUEST, "NotImplemented"),
    }
}

fn read(
    fixture: &Fixture,
    name: &str,
    query: &BTreeMap<String, String>,
    headers: &HeaderMap,
) -> Response {
    let Some((body, generation)) = lock(&fixture.objects).get(name).cloned() else {
        return error(StatusCode::NOT_FOUND, "NoSuchKey");
    };
    // A generation that is no longer live is gone, as in a bucket without versioning.
    if let Some(asked) = query.get("generation") {
        lock(&fixture.generations).push(asked.clone());
        if *asked != generation.to_string() {
            return error(StatusCode::NOT_FOUND, "NoSuchKey");
        }
    }
    let mut common = validators(generation);
    if let Some(expected) = headers.get("if-match").and_then(|v| v.to_str().ok())
        && expected != common[0].1
    {
        return error(StatusCode::PRECONDITION_FAILED, "PreconditionFailed");
    }
    common.push(("accept-ranges", "bytes".to_owned()));
    if let Some(range) = headers.get("range").and_then(|v| v.to_str().ok()) {
        lock(&fixture.ranges).push(range.to_owned());
        let start: usize = range
            .trim_start_matches("bytes=")
            .trim_end_matches('-')
            .parse()
            .unwrap_or(usize::MAX);
        if start >= body.len() {
            return respond(StatusCode::RANGE_NOT_SATISFIABLE, &common, Body::empty());
        }
        common.push((
            "content-range",
            format!("bytes {start}-{}/{}", body.len() - 1, body.len()),
        ));
        common.push(("content-length", (body.len() - start).to_string()));
        return respond(StatusCode::PARTIAL_CONTENT, &common, body[start..].to_vec());
    }
    common.push(("content-length", body.len().to_string()));
    respond(StatusCode::OK, &common, body)
}

fn list(fixture: &Fixture, query: &BTreeMap<String, String>) -> Response {
    let prefix = query.get("prefix").cloned().unwrap_or_default();
    let contents: String = lock(&fixture.objects)
        .iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .map(|(name, (body, generation))| {
            format!(
                "<Contents><Key>{name}</Key><Generation>{generation}</Generation>\
                 <LastModified>2026-09-27T10:00:00.000Z</LastModified>\
                 <ETag>\"g{generation}\"</ETag><Size>{}</Size></Contents>",
                body.len()
            )
        })
        .collect();
    xml(
        StatusCode::OK,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name>\
             <Prefix>{prefix}</Prefix><KeyCount>1</KeyCount><MaxKeys>1000</MaxKeys>\
             <IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
        ),
    )
}

async fn start(source: ObjectCredentialSource, secret: Option<&str>) -> (Harness, Fixture) {
    let fixture = Fixture::default();
    let address = support::serve(Router::new().fallback(handle).with_state(fixture.clone())).await;
    let harness = Harness::start(
        NewObjectStorageProfile {
            name: "gcs".to_owned(),
            provider: ObjectStorageProvider::Gcs,
            endpoint: Some(format!("http://{address}")),
            region: None,
            bucket: Some(BUCKET.to_owned()),
            addressing: ObjectAddressing::Path,
            credential_source: source,
            access_key_id: None,
            account: None,
            secret_ref: None,
            session_token_ref: None,
            checksums: false,
            enabled: true,
            ambient_custom_endpoint: false,
        },
        secret,
        "gs",
        BUCKET,
    )
    .await;
    (harness, fixture)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_bucket_is_listed_and_downloaded_without_any_credential() {
    let (harness, fixture) = start(ObjectCredentialSource::Anonymous, None).await;
    fixture.put("shows/e01.mkv", payload(70_000));
    fixture.put("shows/e02.mkv", payload(10));

    let listing = harness
        .service
        .probe(&harness.link("shows/"))
        .await
        .expect("probe")
        .expect("listing");
    let names: Vec<&str> = listing.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, vec!["e01.mkv", "e02.mkv"]);

    let (file, package) = harness.queue("shows/e01.mkv", "e01.mkv").await;
    let outcome = harness.run(&file, &package).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let written = tokio::fs::read(harness.destination().join("e01.mkv"))
        .await
        .expect("final");
    assert_eq!(written, payload(70_000));
    // Nothing signed, and no application default credentials read on the way.
    assert!(lock(&fixture.authorizations).is_empty());
    // The read asked for exactly the generation the `HEAD` saw.
    assert_eq!(*lock(&fixture.generations), vec!["1".to_owned()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_continues_the_same_generation_and_refuses_a_new_one() {
    let (harness, fixture) = start(ObjectCredentialSource::Anonymous, None).await;
    let body = payload(80_000);
    let generation = fixture.put("shows/e03.mkv", body.clone());
    let validator = format!("\"g{generation}\"; version={generation}");
    let (file, package) = harness.queue("shows/e03.mkv", "e03.mkv").await;
    harness
        .partial(&file, &body[..20_000], 80_000, &validator)
        .await;
    let outcome = harness.run(&file, &package).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert_eq!(*lock(&fixture.ranges), vec!["bytes=20000-".to_owned()]);
    assert_eq!(*lock(&fixture.generations), vec![generation.to_string()]);
    let written = tokio::fs::read(harness.destination().join("e03.mkv"))
        .await
        .expect("final");
    assert_eq!(written, body);

    // The same bytes written again are a new generation: the partial belongs to the old one.
    let (file, package) = harness.queue("shows/e03.mkv", "again.mkv").await;
    harness
        .partial(&file, &body[..20_000], 80_000, &validator)
        .await;
    fixture.put("shows/e03.mkv", body);
    match harness.run(&file, &package).await {
        RunOutcome::Failed(failure) => assert_eq!(failure.code.as_deref(), Some(OBJECT_CHANGED)),
        other => panic!("a new generation must not be resumed, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_multipart_upload_reaches_the_endpoint_and_is_verified() {
    let (harness, fixture) = start(ObjectCredentialSource::Anonymous, None).await;
    let body = payload(20 * 1024 * 1024);
    let report = harness.upload(&body).await;
    assert_eq!(
        report,
        UploadReport::Verified {
            files: vec!["big.bin".to_owned()]
        }
    );
    assert_eq!(fixture.object("release/big.bin"), Some(body));
    assert!(lock(&fixture.uploads).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_throttled_bucket_is_retried_later_under_its_own_code() {
    let (harness, fixture) = start(ObjectCredentialSource::Anonymous, None).await;
    *lock(&fixture.refuse_with) = Some((StatusCode::TOO_MANY_REQUESTS, "rateLimitExceeded"));
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("failure");
    assert_eq!(failure.code.as_deref(), Some(RATE_LIMITED));
    assert!(failure.category.is_retryable());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_that_is_not_a_service_account_key_is_refused_before_any_request() {
    let (harness, fixture) = start(
        ObjectCredentialSource::Static,
        Some("-----BEGIN PRIVATE KEY-----"),
    )
    .await;
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("failure");
    assert_eq!(failure.code.as_deref(), Some(AUTH_FAILED));
    assert!(lock(&fixture.objects).is_empty() && lock(&fixture.authorizations).is_empty());
}

/// A download from a real bucket, when one is named: `RD_GCS_LIVE_BUCKET`, `RD_GCS_LIVE_KEY`
/// (an existing object) and `RD_GCS_LIVE_SERVICE_ACCOUNT` (the path of a key file).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live Google Cloud Storage bucket, named through RD_GCS_LIVE_*"]
async fn a_live_bucket_serves_a_probe() {
    let variable = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (Some(bucket), Some(key), Some(path)) = (
        variable("RD_GCS_LIVE_BUCKET"),
        variable("RD_GCS_LIVE_KEY"),
        variable("RD_GCS_LIVE_SERVICE_ACCOUNT"),
    ) else {
        panic!("RD_GCS_LIVE_BUCKET, RD_GCS_LIVE_KEY and RD_GCS_LIVE_SERVICE_ACCOUNT are needed");
    };
    let secret = std::fs::read_to_string(path).expect("key file");
    let bucket: &'static str = Box::leak(bucket.into_boxed_str());
    let harness = Harness::start(
        NewObjectStorageProfile {
            name: "live".to_owned(),
            provider: ObjectStorageProvider::Gcs,
            endpoint: None,
            region: None,
            bucket: Some(bucket.to_owned()),
            addressing: ObjectAddressing::Path,
            credential_source: ObjectCredentialSource::Static,
            access_key_id: None,
            account: None,
            secret_ref: None,
            session_token_ref: None,
            checksums: false,
            enabled: true,
            ambient_custom_endpoint: false,
        },
        Some(&secret),
        "gs",
        bucket,
    )
    .await;
    let listing = harness
        .service
        .probe(&harness.link(&key))
        .await
        .expect("probe")
        .expect("listing");
    assert!(listing.single_file);
}

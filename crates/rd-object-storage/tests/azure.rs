//! The Azure Blob wire format against a local fixture endpoint (RD-150-05).
//!
//! The fixture answers like Azurite on a path-style endpoint (`http://host/devstoreaccount1`):
//! `HEAD`/`GET` of a blob with `Range` and `If-Match`, the container listing, `Put Block` and
//! `Put Block List`. It trusts every signature and records the `Authorization` header and the
//! query, so the tests can see how a request was signed: with the account key, or with a
//! shared access signature and no header at all.

#![cfg(feature = "azure")]

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
use rd_object_storage::error::{ACCESS_DENIED, AUTH_FAILED, OBJECT_CHANGED};
use rd_scheduler::RunOutcome;
use support::{Harness, decode, lock, payload, respond};

const ACCOUNT: &str = "devstoreaccount1";
/// Azurite's published development key, not a secret of anybody's.
const ACCOUNT_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";
const SAS: &str = "sv=2024-11-04&ss=b&srt=co&sp=rwl&se=2026-10-01T00%3A00%3A00Z&sig=c2lnbmF0dXJl";
const CONTAINER: &str = "media";
const LAST_MODIFIED: &str = "Sun, 27 Sep 2026 10:00:00 GMT";

/// Committed blobs by name: the body and the version its ETag names.
type Blobs = BTreeMap<String, (Vec<u8>, u64)>;
/// Uncommitted blocks by blob and block id.
type Blocks = BTreeMap<(String, String), Vec<u8>>;

#[derive(Clone, Default)]
struct Fixture {
    blobs: Arc<Mutex<Blobs>>,
    /// Uncommitted blocks by blob and block id.
    blocks: Arc<Mutex<Blocks>>,
    next: Arc<Mutex<u64>>,
    authorizations: Arc<Mutex<Vec<String>>>,
    queries: Arc<Mutex<Vec<String>>>,
    ranges: Arc<Mutex<Vec<String>>>,
    /// Answers every request with this Azure error code and 403.
    refuse_with: Arc<Mutex<Option<&'static str>>>,
}

impl Fixture {
    fn put(&self, name: &str, body: Vec<u8>) {
        let mut next = lock(&self.next);
        *next += 1;
        lock(&self.blobs).insert(name.to_owned(), (body, *next));
    }

    fn blob(&self, name: &str) -> Option<Vec<u8>> {
        lock(&self.blobs).get(name).map(|(body, _)| body.clone())
    }

    fn etag(&self, name: &str) -> String {
        let version = lock(&self.blobs)
            .get(name)
            .map_or(0, |(_, version)| *version);
        etag(version)
    }
}

/// The ETag of a blob version; free of the fixture so a caller already holding `blobs` can
/// use it — the mutex is not reentrant.
fn etag(version: u64) -> String {
    format!("\"0x8D{version:013X}\"")
}

fn xml(status: StatusCode, body: String) -> Response {
    respond(
        status,
        &[("content-type", "application/xml".to_owned())],
        body,
    )
}

fn error(status: StatusCode, code: &str) -> Response {
    xml(
        status,
        format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><Error><Code>{code}</Code></Error>"),
    )
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
    lock(&fixture.queries).push(uri.query().unwrap_or("").to_owned());
    if let Some(code) = *lock(&fixture.refuse_with) {
        return error(StatusCode::FORBIDDEN, code);
    }
    let path = uri.path().trim_start_matches('/');
    let Some(rest) = path.strip_prefix(&format!("{ACCOUNT}/")) else {
        return error(StatusCode::BAD_REQUEST, "InvalidUri");
    };
    let (container, blob) = rest.split_once('/').unwrap_or((rest, ""));
    if container != CONTAINER {
        return error(StatusCode::NOT_FOUND, "ContainerNotFound");
    }
    let blob = decode(blob);
    let query: BTreeMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    match (method, query.get("comp").map(String::as_str)) {
        (Method::GET, Some("list")) => list(&fixture, &query),
        (Method::HEAD | Method::GET, None) => read(&fixture, &blob, &headers),
        (Method::PUT, Some("block")) => {
            let id = query.get("blockid").cloned().unwrap_or_default();
            lock(&fixture.blocks).insert((blob, id), body.to_vec());
            respond(StatusCode::CREATED, &[], Body::empty())
        }
        (Method::PUT, Some("blocklist")) => {
            let listed = String::from_utf8_lossy(&body).into_owned();
            let mut content = Vec::new();
            for id in listed.split("<Uncommitted>").skip(1) {
                let id = id
                    .split("</Uncommitted>")
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let Some(block) = lock(&fixture.blocks).remove(&(blob.clone(), id)) else {
                    return error(StatusCode::BAD_REQUEST, "InvalidBlockList");
                };
                content.extend(block);
            }
            fixture.put(&blob, content);
            respond(
                StatusCode::CREATED,
                &[("etag", fixture.etag(&blob))],
                Body::empty(),
            )
        }
        (Method::PUT, None) => {
            fixture.put(&blob, body.to_vec());
            respond(
                StatusCode::CREATED,
                &[("etag", fixture.etag(&blob))],
                Body::empty(),
            )
        }
        _ => error(StatusCode::BAD_REQUEST, "UnsupportedHttpVerb"),
    }
}

fn read(fixture: &Fixture, name: &str, headers: &HeaderMap) -> Response {
    let Some((body, _)) = lock(&fixture.blobs).get(name).cloned() else {
        return error(StatusCode::NOT_FOUND, "BlobNotFound");
    };
    let etag = fixture.etag(name);
    if let Some(expected) = headers.get("if-match").and_then(|v| v.to_str().ok())
        && expected != etag
    {
        return error(StatusCode::PRECONDITION_FAILED, "ConditionNotMet");
    }
    let mut common = vec![
        ("etag", etag),
        ("last-modified", LAST_MODIFIED.to_owned()),
        ("accept-ranges", "bytes".to_owned()),
    ];
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
    let blobs: String = lock(&fixture.blobs)
        .iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .map(|(name, (body, version))| {
            format!(
                "<Blob><Name>{name}</Name><Properties><Last-Modified>{LAST_MODIFIED}\
                 </Last-Modified><Etag>{}</Etag><Content-Length>{}</Content-Length>\
                 <Content-Type>application/octet-stream</Content-Type><BlobType>BlockBlob\
                 </BlobType></Properties></Blob>",
                etag(*version),
                body.len()
            )
        })
        .collect();
    xml(
        StatusCode::OK,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><EnumerationResults \
             ServiceEndpoint=\"http://127.0.0.1/{ACCOUNT}\" ContainerName=\"{CONTAINER}\">\
             <Prefix>{prefix}</Prefix><MaxResults>5000</MaxResults><Blobs>{blobs}</Blobs>\
             <NextMarker/></EnumerationResults>"
        ),
    )
}

async fn start(source: ObjectCredentialSource, secret: &str) -> (Harness, Fixture) {
    let fixture = Fixture::default();
    let address = support::serve(Router::new().fallback(handle).with_state(fixture.clone())).await;
    let harness = Harness::start(
        NewObjectStorageProfile {
            name: "azurite".to_owned(),
            provider: ObjectStorageProvider::Azure,
            endpoint: Some(format!("http://{address}/{ACCOUNT}")),
            region: None,
            bucket: Some(CONTAINER.to_owned()),
            addressing: ObjectAddressing::Path,
            credential_source: source,
            access_key_id: None,
            account: Some(ACCOUNT.to_owned()),
            secret_ref: None,
            session_token_ref: None,
            checksums: false,
            enabled: true,
        },
        Some(secret),
        "az",
        CONTAINER,
    )
    .await;
    (harness, fixture)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_container_is_listed_and_a_blob_downloaded_signed_with_the_account_key() {
    let (harness, fixture) = start(ObjectCredentialSource::Static, ACCOUNT_KEY).await;
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
    assert!(listing.entries.iter().all(|entry| entry.etag.is_some()));

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
    let authorizations = lock(&fixture.authorizations).clone();
    assert!(!authorizations.is_empty());
    assert!(
        authorizations
            .iter()
            .all(|value| value.starts_with(&format!("SharedKey {ACCOUNT}:"))),
        "{authorizations:?}"
    );
    // The key signs; it never travels.
    assert!(
        authorizations
            .iter()
            .all(|value| !value.contains(ACCOUNT_KEY))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_access_signature_travels_as_the_query_and_never_as_a_header() {
    let (harness, fixture) = start(
        ObjectCredentialSource::SharedAccessSignature,
        &format!("https://{ACCOUNT}.blob.core.windows.net/{CONTAINER}?{SAS}"),
    )
    .await;
    fixture.put("a.bin", payload(5_000));
    let (file, package) = harness.queue("a.bin", "a.bin").await;
    let outcome = harness.run(&file, &package).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert!(lock(&fixture.authorizations).is_empty());
    let queries = lock(&fixture.queries).clone();
    assert!(!queries.is_empty());
    assert!(
        queries
            .iter()
            .all(|query| query.contains("sig=c2lnbmF0dXJl") && query.contains("sv=2024-11-04")),
        "{queries:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_asks_for_the_rest_of_the_same_blob_only() {
    let (harness, fixture) = start(ObjectCredentialSource::Static, ACCOUNT_KEY).await;
    let body = payload(80_000);
    fixture.put("shows/e03.mkv", body.clone());
    let (file, package) = harness.queue("shows/e03.mkv", "e03.mkv").await;
    harness
        .partial(
            &file,
            &body[..20_000],
            80_000,
            &fixture.etag("shows/e03.mkv"),
        )
        .await;

    let outcome = harness.run(&file, &package).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert_eq!(*lock(&fixture.ranges), vec!["bytes=20000-".to_owned()]);
    let written = tokio::fs::read(harness.destination().join("e03.mkv"))
        .await
        .expect("final");
    assert_eq!(written, body);

    // Overwritten under the same name: the next partial of it is refused, not continued.
    let (file, package) = harness.queue("shows/e03.mkv", "again.mkv").await;
    harness
        .partial(
            &file,
            &body[..20_000],
            80_000,
            &fixture.etag("shows/e03.mkv"),
        )
        .await;
    fixture.put("shows/e03.mkv", body);
    match harness.run(&file, &package).await {
        RunOutcome::Failed(failure) => assert_eq!(failure.code.as_deref(), Some(OBJECT_CHANGED)),
        other => panic!("an overwritten blob must not be resumed, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_block_upload_reaches_the_endpoint_and_is_verified() {
    let (harness, fixture) = start(ObjectCredentialSource::Static, ACCOUNT_KEY).await;
    let body = payload(20 * 1024 * 1024);
    let report = harness.upload(&body).await;
    assert_eq!(
        report,
        UploadReport::Verified {
            files: vec!["big.bin".to_owned()]
        }
    );
    assert_eq!(fixture.blob("release/big.bin"), Some(body));
    assert!(lock(&fixture.blocks).is_empty(), "every block committed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_key_is_an_authentication_failure_and_a_missing_right_is_not() {
    let (harness, fixture) = start(ObjectCredentialSource::Static, ACCOUNT_KEY).await;
    *lock(&fixture.refuse_with) = Some("AuthenticationFailed");
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("failure");
    assert_eq!(failure.code.as_deref(), Some(AUTH_FAILED));
    *lock(&fixture.refuse_with) = Some("AuthorizationPermissionMismatch");
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("failure");
    assert_eq!(failure.code.as_deref(), Some(ACCESS_DENIED));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_that_is_not_base64_is_refused_before_any_request() {
    let (harness, fixture) = start(ObjectCredentialSource::Static, "not a key!").await;
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("failure");
    assert_eq!(failure.code.as_deref(), Some(AUTH_FAILED));
    assert!(lock(&fixture.queries).is_empty());
}

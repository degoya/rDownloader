//! The S3 wire format against a local fixture endpoint (RD-150-04).
//!
//! `src/tests.rs` holds the provider-neutral contract against memory. This file checks what
//! only a real HTTP exchange shows: path-style addressing, a signed request, `Range` with
//! `If-Match` on a resume, the `ListObjectsV2` answer and the multipart calls. The fixture is
//! deliberately small — it keeps objects and parts in memory and trusts every signature,
//! recording the `Authorization` header so the test can check a request was signed at all.
//!
//! `RD_S3_LIVE_*` runs the same download against a real service (MinIO, AWS); see the ignored
//! test at the end.

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
use rd_core::{
    DownloadKind, DownloadState, ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider,
    PackageId,
};
use rd_db::{Database, NewDownload, NewObjectStorageProfile, NewPackage};
use rd_extract::{ObjectUpload, ObjectUploader, UploadReport};
use rd_object_storage::ObjectStorageService;
use rd_scheduler::{RunLimits, RunOutcome};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const BUCKET: &str = "media-bucket";
const LAST_MODIFIED: &str = "Sun, 27 Sep 2026 10:00:00 GMT";

/// Stored objects by key: the body and the version its ETag names.
type Objects = BTreeMap<String, (Vec<u8>, u64)>;
/// Open multipart uploads by id: the parts received so far, by part number.
type Uploads = BTreeMap<String, BTreeMap<u32, Vec<u8>>>;

#[derive(Clone, Default)]
struct Fixture {
    objects: Arc<Mutex<Objects>>,
    uploads: Arc<Mutex<Uploads>>,
    next: Arc<Mutex<u64>>,
    authorizations: Arc<Mutex<Vec<String>>>,
    ranges: Arc<Mutex<Vec<String>>>,
    /// Answers every request with this S3 error code and 403.
    refuse_with: Arc<Mutex<Option<&'static str>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Fixture {
    fn put(&self, key: &str, body: Vec<u8>) {
        let mut next = lock(&self.next);
        *next += 1;
        lock(&self.objects).insert(key.to_owned(), (body, *next));
    }

    fn object(&self, key: &str) -> Option<Vec<u8>> {
        lock(&self.objects).get(key).map(|(body, _)| body.clone())
    }
}

fn respond(status: StatusCode, headers: &[(&str, String)], body: impl Into<Body>) -> Response {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    builder
        .body(body.into())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn xml(status: StatusCode, body: String) -> Response {
    respond(
        status,
        &[("content-type", "application/xml".to_owned())],
        body,
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
    if let Some(code) = *lock(&fixture.refuse_with) {
        return xml(
            StatusCode::FORBIDDEN,
            format!("<Error><Code>{code}</Code><Message>refused</Message></Error>"),
        );
    }
    let path = uri.path().trim_start_matches('/');
    let (bucket, key) = path.split_once('/').unwrap_or((path, ""));
    if bucket != BUCKET {
        return xml(
            StatusCode::NOT_FOUND,
            "<Error><Code>NoSuchBucket</Code></Error>".to_owned(),
        );
    }
    let query: BTreeMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    match method {
        Method::GET if key.is_empty() => list(&fixture, &query),
        Method::HEAD | Method::GET => read(&fixture, key, &headers, method == Method::HEAD),
        Method::PUT if query.contains_key("uploadId") => {
            let number: u32 = query
                .get("partNumber")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let mut uploads = lock(&fixture.uploads);
            let Some(upload) = uploads.get_mut(&query["uploadId"]) else {
                return xml(
                    StatusCode::NOT_FOUND,
                    "<Error><Code>NoSuchUpload</Code></Error>".to_owned(),
                );
            };
            upload.insert(number, body.to_vec());
            respond(
                StatusCode::OK,
                &[("etag", format!("\"part-{number}\""))],
                Body::empty(),
            )
        }
        Method::PUT => {
            fixture.put(key, body.to_vec());
            let version = lock(&fixture.objects)
                .get(key)
                .map_or(0, |(_, version)| *version);
            respond(
                StatusCode::OK,
                &[("etag", format!("\"v{version}\""))],
                Body::empty(),
            )
        }
        Method::POST if query.contains_key("uploads") => {
            let id = {
                let mut next = lock(&fixture.next);
                *next += 1;
                format!("upload-{next}")
            };
            lock(&fixture.uploads).insert(id.clone(), BTreeMap::new());
            xml(
                StatusCode::OK,
                format!(
                    "<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key>\
                     <UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
                ),
            )
        }
        Method::POST if query.contains_key("uploadId") => {
            let Some(parts) = lock(&fixture.uploads).remove(&query["uploadId"]) else {
                return xml(
                    StatusCode::NOT_FOUND,
                    "<Error><Code>NoSuchUpload</Code></Error>".to_owned(),
                );
            };
            fixture.put(key, parts.into_values().flatten().collect());
            xml(
                StatusCode::OK,
                format!(
                    "<CompleteMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key>\
                     <ETag>\"done\"</ETag></CompleteMultipartUploadResult>"
                ),
            )
        }
        Method::DELETE if query.contains_key("uploadId") => {
            lock(&fixture.uploads).remove(&query["uploadId"]);
            respond(StatusCode::NO_CONTENT, &[], Body::empty())
        }
        _ => xml(
            StatusCode::BAD_REQUEST,
            "<Error><Code>NotImplemented</Code></Error>".to_owned(),
        ),
    }
}

fn read(fixture: &Fixture, key: &str, headers: &HeaderMap, head: bool) -> Response {
    let Some((body, version)) = lock(&fixture.objects).get(key).cloned() else {
        return xml(
            StatusCode::NOT_FOUND,
            "<Error><Code>NoSuchKey</Code></Error>".to_owned(),
        );
    };
    let etag = format!("\"v{version}\"");
    if let Some(expected) = headers.get("if-match").and_then(|v| v.to_str().ok())
        && expected != etag
    {
        return xml(
            StatusCode::PRECONDITION_FAILED,
            "<Error><Code>PreconditionFailed</Code></Error>".to_owned(),
        );
    }
    let mut common = vec![
        ("etag", etag),
        ("last-modified", LAST_MODIFIED.to_owned()),
        ("accept-ranges", "bytes".to_owned()),
    ];
    let range = headers
        .get("range")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if let Some(range) = range.filter(|_| !head) {
        lock(&fixture.ranges).push(range.clone());
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
    if head {
        return respond(StatusCode::OK, &common, Body::empty());
    }
    respond(StatusCode::OK, &common, body)
}

fn list(fixture: &Fixture, query: &BTreeMap<String, String>) -> Response {
    let prefix = query.get("prefix").cloned().unwrap_or_default();
    let contents: String = lock(&fixture.objects)
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(key, (body, version))| {
            format!(
                "<Contents><Key>{key}</Key><LastModified>2026-09-27T10:00:00.000Z</LastModified>\
                 <ETag>\"v{version}\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass>\
                 </Contents>",
                body.len()
            )
        })
        .collect();
    xml(
        StatusCode::OK,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name>\
             <Prefix>{prefix}</Prefix><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>\
             {contents}</ListBucketResult>"
        ),
    )
}

struct Harness {
    directory: tempfile::TempDir,
    database: Database,
    service: ObjectStorageService,
    fixture: Fixture,
    profile: rd_core::ObjectStorageProfile,
}

impl Harness {
    async fn start() -> Self {
        let fixture = Fixture::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        // A part is at least 16 MiB; axum's default 2 MiB body limit would refuse every one.
        let app = Router::new()
            .fallback(handle)
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(fixture.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("objects.sqlite3"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secrets");
        let secret_ref = secrets
            .put_string("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned())
            .await
            .expect("secret");
        let profile = database
            .create_object_storage_profile(NewObjectStorageProfile {
                name: "fixture".to_owned(),
                provider: ObjectStorageProvider::S3,
                endpoint: Some(format!("http://{address}")),
                region: Some("us-east-1".to_owned()),
                bucket: Some(BUCKET.to_owned()),
                addressing: ObjectAddressing::Path,
                credential_source: ObjectCredentialSource::Static,
                access_key_id: Some("AKIDEXAMPLE".to_owned()),
                account: None,
                secret_ref: Some(secret_ref),
                session_token_ref: None,
                checksums: false,
                enabled: true,
                ambient_custom_endpoint: false,
            })
            .await
            .expect("profile");
        let service = ObjectStorageService::new(
            database.clone(),
            secrets,
            Arc::new(RwLock::new(rd_core::RemoteSettings::default())),
            Arc::new(RwLock::new(rd_http::NetworkDefaults::default())),
        );
        Self {
            directory,
            database,
            service,
            fixture,
            profile,
        }
    }

    fn destination(&self) -> std::path::PathBuf {
        self.directory.path().join("downloads")
    }

    async fn queue(
        &self,
        key: &str,
        name: &str,
    ) -> (rd_core::DownloadFile, rd_core::DownloadPackage) {
        let package = self
            .database
            .create_package(NewPackage {
                id: PackageId::new(),
                name: "objects".to_owned(),
                destination: self.destination().to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let file = self
            .database
            .create_download(NewDownload {
                id: rd_core::DownloadId::new(),
                package_id: package.id,
                source: format!("s3://{BUCKET}/{key}").parse().expect("url"),
                file_name: name.to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::ObjectStorage,
                media: None,
                remote_credential_id: None,
                mirror_group: None,
                replay: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        (file, package)
    }

    async fn run(
        &self,
        file: &rd_core::DownloadFile,
        package: &rd_core::DownloadPackage,
    ) -> RunOutcome {
        rd_object_storage::build(self.service.clone())
            .run(
                file,
                package,
                CancellationToken::new(),
                RunLimits {
                    max_parallel_requests: 0,
                    bandwidth: rd_limits::ScopedLimiter::unlimited(),
                    address_policy: None,
                },
            )
            .await
            .expect("run")
    }
}

fn payload(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 253) as u8).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_object_is_listed_and_downloaded_with_signed_path_style_requests() {
    let harness = Harness::start().await;
    harness.fixture.put("shows/e01.mkv", payload(70_000));
    harness.fixture.put("shows/e02.mkv", payload(10));

    let listing = harness
        .service
        .probe(&format!("s3://{BUCKET}/shows/").parse().expect("url"))
        .await
        .expect("probe")
        .expect("listing");
    let names: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
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
    let authorizations = lock(&harness.fixture.authorizations).clone();
    assert!(!authorizations.is_empty());
    assert!(
        authorizations
            .iter()
            .all(|value| value.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/")),
        "{authorizations:?}"
    );
    // The secret signs; it never travels.
    assert!(
        authorizations
            .iter()
            .all(|value| !value.contains("wJalrXUtnFEMI"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_asks_for_the_rest_of_the_same_version_only() {
    let harness = Harness::start().await;
    let body = payload(80_000);
    harness.fixture.put("shows/e03.mkv", body.clone());
    let (file, package) = harness.queue("shows/e03.mkv", "e03.mkv").await;

    // An earlier attempt left 20 000 bytes and the object's validators behind.
    let root = rd_files::StorageRoot::create(
        rd_core::StorageRootId::new(),
        "download destination".to_owned(),
        harness.destination(),
    )
    .await
    .expect("root");
    tokio::fs::create_dir_all(root.path()).await.expect("dir");
    let part = rd_files::part_path(&root, file.id).await.expect("part");
    tokio::fs::write(&part, &body[..20_000])
        .await
        .expect("partial");
    harness
        .database
        .prepare_transfer(
            file.id,
            Some(80_000),
            Some("\"v1\"".to_owned()),
            None,
            Vec::new(),
        )
        .await
        .expect("validators");

    let outcome = harness.run(&file, &package).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        *lock(&harness.fixture.ranges),
        vec!["bytes=20000-".to_owned()]
    );
    let written = tokio::fs::read(harness.destination().join("e03.mkv"))
        .await
        .expect("final");
    assert_eq!(written, body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_multipart_upload_reaches_the_endpoint_and_is_verified() {
    let harness = Harness::start().await;
    let directory = harness.directory.path().join("finished");
    tokio::fs::create_dir_all(&directory).await.expect("dir");
    let body = payload(20 * 1024 * 1024);
    tokio::fs::write(directory.join("big.bin"), &body)
        .await
        .expect("file");
    let files = vec!["big.bin".to_owned()];
    let report = harness
        .service
        .upload(
            &harness.profile.id.to_string(),
            ObjectUpload {
                owner: "package-1",
                package_name: "release",
                directory: &directory,
                files: &files,
                destination: "",
                progress: Arc::new(|_, _| {}),
                stop: CancellationToken::new(),
                bandwidth: rd_limits::ScopedLimiter::unlimited(),
            },
        )
        .await
        .expect("upload");
    assert_eq!(report, UploadReport::Verified { files });
    assert_eq!(harness.fixture.object("release/big.bin"), Some(body));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_keeps_the_upload_limit() {
    // RD-150-15, measured against the endpoint: 2 MiB at 1 MiB/s. The limiter's bucket holds
    // one second's worth, so the second mebibyte is the paced one.
    const RATE: u64 = 1024 * 1024;
    let harness = Harness::start().await;
    let directory = harness.directory.path().join("finished");
    tokio::fs::create_dir_all(&directory).await.expect("dir");
    let body = payload(2 * RATE as usize);
    tokio::fs::write(directory.join("paced.bin"), &body)
        .await
        .expect("file");
    let files = vec!["paced.bin".to_owned()];
    let limits = rd_limits::LimiterRegistry::new();
    limits.apply_upload(Some(RATE));
    let started = std::time::Instant::now();
    let report = harness
        .service
        .upload(
            &harness.profile.id.to_string(),
            ObjectUpload {
                owner: "package-paced",
                package_name: "release",
                directory: &directory,
                files: &files,
                destination: "",
                progress: Arc::new(|_, _| {}),
                stop: CancellationToken::new(),
                bandwidth: limits.upload(),
            },
        )
        .await
        .expect("upload");
    let elapsed = started.elapsed().as_secs_f64();
    assert_eq!(report, UploadReport::Verified { files });
    assert_eq!(harness.fixture.object("release/paced.bin"), Some(body));
    let rate = (2 * RATE - RATE) as f64 / elapsed;
    assert!(
        rate <= RATE as f64 * 1.05,
        "{rate:.0} B/s past the burst, {elapsed:.2} s in all"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_signature_is_an_authentication_failure() {
    let harness = Harness::start().await;
    *lock(&harness.fixture.refuse_with) = Some("SignatureDoesNotMatch");
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("failure");
    assert_eq!(
        failure.code.as_deref(),
        Some(rd_object_storage::error::AUTH_FAILED)
    );
    *lock(&harness.fixture.refuse_with) = Some("AccessDenied");
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("failure");
    assert_eq!(
        failure.code.as_deref(),
        Some(rd_object_storage::error::ACCESS_DENIED)
    );
}

/// The same download against a real service, when one is named:
/// `RD_S3_LIVE_ENDPOINT` (empty for AWS), `RD_S3_LIVE_REGION`, `RD_S3_LIVE_BUCKET`,
/// `RD_S3_LIVE_KEY` (an existing object), `RD_S3_LIVE_ACCESS_KEY`, `RD_S3_LIVE_SECRET`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live S3 service, named through RD_S3_LIVE_*"]
async fn a_live_service_serves_a_download() {
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let (Some(bucket), Some(key), Some(access), Some(secret)) = (
        variable("RD_S3_LIVE_BUCKET"),
        variable("RD_S3_LIVE_KEY"),
        variable("RD_S3_LIVE_ACCESS_KEY"),
        variable("RD_S3_LIVE_SECRET"),
    ) else {
        panic!(
            "set RD_S3_LIVE_BUCKET, RD_S3_LIVE_KEY, RD_S3_LIVE_ACCESS_KEY and RD_S3_LIVE_SECRET"
        );
    };
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("live.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    let endpoint = variable("RD_S3_LIVE_ENDPOINT");
    database
        .create_object_storage_profile(NewObjectStorageProfile {
            name: "live".to_owned(),
            provider: ObjectStorageProvider::S3,
            addressing: ObjectAddressing::default_for(endpoint.as_deref()),
            endpoint,
            region: variable("RD_S3_LIVE_REGION"),
            bucket: Some(bucket.clone()),
            credential_source: ObjectCredentialSource::Static,
            access_key_id: Some(access),
            account: None,
            secret_ref: Some(secrets.put_string(secret).await.expect("secret")),
            session_token_ref: None,
            checksums: true,
            enabled: true,
            ambient_custom_endpoint: false,
        })
        .await
        .expect("profile");
    let service = ObjectStorageService::new(
        database.clone(),
        secrets,
        Arc::new(RwLock::new(rd_core::RemoteSettings::default())),
        Arc::new(RwLock::new(rd_http::NetworkDefaults::default())),
    );
    let listing = service
        .probe(&format!("s3://{bucket}/{key}").parse().expect("url"))
        .await
        .expect("probe")
        .expect("listing");
    assert!(listing.single_file);
}

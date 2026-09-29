//! The object storage contract, run against memory (RD-150-04, RD-150-05).
//!
//! What every provider has to satisfy — ranged resume guarded by the version, the prefix
//! listing, the multipart upload that continues after a restart and aborts what no longer
//! fits — without a server. `every_provider_meets_the_download_contract` runs the download
//! half with Azure Blob and GCS profiles and links. `tests/s3.rs`, `tests/azure.rs` and
//! `tests/gcs.rs` check each wire format against a fixture server.

use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use object_store::{
    MultipartId, ObjectStore, PutOptions, PutPayload, PutResult,
    memory::InMemory,
    multipart::{MultipartStore, PartId},
    path::Path,
};
use rd_core::{
    DownloadKind, DownloadState, ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider,
    PackageId,
};
use rd_db::{Database, NewDownload, NewObjectStorageProfile, NewPackage};
use rd_extract::{ObjectUpload, ObjectUploader, UploadReport};
use rd_scheduler::{ExternalRunner, RunLimits, RunOutcome};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{ObjectStorageService, connect::Store};

/// The multipart half of a store, kept in memory, which finishes into the same [`InMemory`].
#[derive(Debug, Default)]
struct Parts {
    target: Arc<InMemory>,
    uploads: Mutex<BTreeMap<String, BTreeMap<usize, Vec<u8>>>>,
    next: Mutex<u64>,
    /// Refuses the part with this index once, the way a dropped connection does.
    fail_part: Mutex<Option<usize>>,
    sent: Mutex<Vec<usize>>,
    aborted: Mutex<HashSet<String>>,
}

impl std::fmt::Display for Parts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Parts")
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[async_trait]
impl MultipartStore for Parts {
    async fn create_multipart(&self, _path: &Path) -> object_store::Result<MultipartId> {
        let mut next = lock(&self.next);
        *next += 1;
        let id = format!("mpu-{next}");
        lock(&self.uploads).insert(id.clone(), BTreeMap::new());
        Ok(id)
    }

    async fn put_part(
        &self,
        _path: &Path,
        id: &MultipartId,
        part_idx: usize,
        data: PutPayload,
    ) -> object_store::Result<PartId> {
        if lock(&self.fail_part)
            .take_if(|index| *index == part_idx)
            .is_some()
        {
            return Err(object_store::Error::Generic {
                store: "Parts",
                source: "connection reset".into(),
            });
        }
        let bytes: Vec<u8> = data
            .iter()
            .flat_map(|chunk| chunk.iter().copied())
            .collect();
        let mut uploads = lock(&self.uploads);
        let Some(parts) = uploads.get_mut(id) else {
            return Err(object_store::Error::NotFound {
                path: id.clone(),
                source: "no such upload".into(),
            });
        };
        parts.insert(part_idx, bytes);
        lock(&self.sent).push(part_idx);
        Ok(PartId {
            content_id: format!("\"etag-{part_idx}\""),
        })
    }

    async fn complete_multipart(
        &self,
        path: &Path,
        id: &MultipartId,
        parts: Vec<PartId>,
    ) -> object_store::Result<PutResult> {
        let stored =
            lock(&self.uploads)
                .remove(id)
                .ok_or_else(|| object_store::Error::NotFound {
                    path: id.clone(),
                    source: "no such upload".into(),
                })?;
        assert_eq!(
            parts.len(),
            stored.len(),
            "every part is named at completion"
        );
        let body: Vec<u8> = stored.into_values().flatten().collect();
        self.target
            .put_opts(path, PutPayload::from(body), PutOptions::default())
            .await
    }

    async fn abort_multipart(&self, _path: &Path, id: &MultipartId) -> object_store::Result<()> {
        lock(&self.uploads).remove(id);
        lock(&self.aborted).insert(id.clone());
        Ok(())
    }
}

struct Harness {
    directory: tempfile::TempDir,
    database: Database,
    service: ObjectStorageService,
    memory: Arc<InMemory>,
    parts: Arc<Parts>,
    /// The provider of the fixture profile and the scheme of the links queued.
    provider: ObjectStorageProvider,
}

impl Harness {
    async fn start() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("objects.sqlite3"))
            .await
            .expect("database");
        Self::with_database(directory, database).await
    }

    async fn start_for(provider: ObjectStorageProvider) -> Self {
        Self {
            provider,
            ..Self::start().await
        }
    }

    fn link(&self, key: &str) -> url::Url {
        format!("{}://media-bucket/{key}", self.provider.scheme())
            .parse()
            .expect("url")
    }

    /// A second process on the same files: a new service, the same database and bucket.
    async fn restart(self) -> Self {
        let Self {
            directory,
            memory,
            parts,
            provider,
            ..
        } = self;
        let database = Database::open(directory.path().join("objects.sqlite3"))
            .await
            .expect("database");
        let mut harness = Self::with_database(directory, database).await;
        harness.memory = memory.clone();
        harness.parts = parts.clone();
        harness.provider = provider;
        harness.service.fixture = Some(Store {
            objects: memory,
            parts,
        });
        harness
    }

    async fn with_database(directory: tempfile::TempDir, database: Database) -> Self {
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secrets");
        let mut service = ObjectStorageService::new(
            database.clone(),
            secrets,
            Arc::new(RwLock::new(rd_core::RemoteSettings::default())),
            Arc::new(RwLock::new(rd_http::NetworkDefaults::default())),
        );
        let memory = Arc::new(InMemory::new());
        let parts = Arc::new(Parts {
            target: memory.clone(),
            ..Parts::default()
        });
        service.fixture = Some(Store {
            objects: memory.clone(),
            parts: parts.clone(),
        });
        Self {
            directory,
            database,
            service,
            memory,
            parts,
            provider: ObjectStorageProvider::S3,
        }
    }

    async fn profile(&self) -> rd_core::ObjectStorageProfile {
        if let Some(existing) = self
            .database
            .list_object_storage_profiles()
            .await
            .expect("profiles")
            .into_iter()
            .next()
        {
            return existing;
        }
        self.database
            .create_object_storage_profile(NewObjectStorageProfile {
                name: "fixture".to_owned(),
                provider: self.provider,
                endpoint: Some("http://127.0.0.1:9".to_owned()),
                region: None,
                bucket: Some("media-bucket".to_owned()),
                addressing: ObjectAddressing::Path,
                credential_source: ObjectCredentialSource::Anonymous,
                access_key_id: None,
                account: (self.provider == ObjectStorageProvider::Azure)
                    .then(|| "devstoreaccount1".to_owned()),
                secret_ref: None,
                session_token_ref: None,
                checksums: true,
                enabled: true,
            })
            .await
            .expect("profile")
    }

    async fn put(&self, key: &str, body: Vec<u8>) {
        self.memory
            .put_opts(
                &Path::from(key),
                PutPayload::from(body),
                PutOptions::default(),
            )
            .await
            .expect("put");
    }

    fn destination(&self) -> std::path::PathBuf {
        self.directory.path().join("downloads")
    }

    async fn queue(&self, key: &str, name: &str) -> rd_core::DownloadFile {
        let package_id = PackageId::new();
        self.database
            .create_package(NewPackage {
                id: package_id,
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
        self.database
            .create_download(NewDownload {
                id: rd_core::DownloadId::new(),
                package_id,
                source: self.link(key),
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
            .expect("download")
    }

    async fn run(&self, file: &rd_core::DownloadFile) -> RunOutcome {
        let package = self
            .database
            .list_packages()
            .await
            .expect("packages")
            .into_iter()
            .find(|package| package.id == file.package_id)
            .expect("package");
        crate::ObjectStorageRunner::new(self.service.clone())
            .run(
                file,
                &package,
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

    /// Leaves the first `bytes` of the object behind as an earlier attempt would, with the
    /// validators that attempt recorded.
    async fn partial(&self, file: &rd_core::DownloadFile, body: &[u8], etag: Option<String>) {
        let root = rd_files::StorageRoot::create(
            rd_core::StorageRootId::new(),
            "download destination".to_owned(),
            self.destination(),
        )
        .await
        .expect("root");
        tokio::fs::create_dir_all(root.path()).await.expect("dir");
        let part = rd_files::part_path(&root, file.id).await.expect("part");
        tokio::fs::write(&part, body).await.expect("partial");
        self.database
            .prepare_transfer(file.id, Some(body.len() as u64 * 4), etag, None, Vec::new())
            .await
            .expect("validators");
    }
}

fn payload(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 251) as u8).collect()
}

async fn etag_of(harness: &Harness, key: &str) -> Option<String> {
    crate::head(
        &Store {
            objects: harness.memory.clone(),
            parts: harness.parts.clone(),
        },
        &Path::from(key),
    )
    .await
    .expect("head")
    .e_tag
}

#[tokio::test]
async fn an_object_downloads_in_full() {
    let harness = Harness::start().await;
    harness.profile().await;
    harness.put("shows/e01.mkv", payload(300_000)).await;
    let file = harness.queue("shows/e01.mkv", "e01.mkv").await;
    let outcome = harness.run(&file).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let written = tokio::fs::read(harness.destination().join("e01.mkv"))
        .await
        .expect("final");
    assert_eq!(written, payload(300_000));
}

#[tokio::test]
async fn an_interrupted_download_continues_behind_the_partial_file() {
    let harness = Harness::start().await;
    harness.profile().await;
    let body = payload(400_000);
    harness.put("shows/e02.mkv", body.clone()).await;
    let file = harness.queue("shows/e02.mkv", "e02.mkv").await;
    // A quarter on disk, recorded against the object's current version. The size recorded
    // is the object's; `partial` derives it from the quarter.
    harness
        .partial(
            &file,
            &body[..100_000],
            etag_of(&harness, "shows/e02.mkv").await,
        )
        .await;
    let outcome = harness.run(&file).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let written = tokio::fs::read(harness.destination().join("e02.mkv"))
        .await
        .expect("final");
    // The whole object, not a restart appended behind the quarter.
    assert_eq!(written, body);
}

#[tokio::test]
async fn a_replaced_object_refuses_the_resume_and_keeps_the_partial() {
    let harness = Harness::start().await;
    harness.profile().await;
    let body = payload(400_000);
    harness.put("shows/e03.mkv", body.clone()).await;
    let file = harness.queue("shows/e03.mkv", "e03.mkv").await;
    harness
        .partial(
            &file,
            &body[..100_000],
            etag_of(&harness, "shows/e03.mkv").await,
        )
        .await;
    // Same key, same size, new version.
    harness.put("shows/e03.mkv", body).await;
    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(crate::error::OBJECT_CHANGED));
        }
        other => panic!("a replaced object must not be resumed, got {other:?}"),
    }
    assert!(!harness.destination().join("e03.mkv").exists());
}

#[tokio::test]
async fn a_missing_object_is_reported_with_its_bucket() {
    let harness = Harness::start().await;
    harness.profile().await;
    let file = harness.queue("shows/none.mkv", "none.mkv").await;
    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(crate::error::NOT_FOUND));
            assert_eq!(
                failure.params.get("bucket").map(String::as_str),
                Some("media-bucket")
            );
        }
        other => panic!("expected a missing object, got {other:?}"),
    }
}

#[tokio::test]
async fn a_key_without_its_slash_is_listed_as_the_prefix_it_means() {
    let harness = Harness::start().await;
    harness.profile().await;
    harness.put("shows/s1/e01.mkv", payload(10)).await;
    harness.put("shows/s1/e02.mkv", payload(20)).await;
    harness.put("shows/extra.nfo", payload(5)).await;
    harness.put("other/x.bin", payload(5)).await;
    let listing = harness
        .service
        .probe(&"s3://media-bucket/shows".parse().expect("url"))
        .await
        .expect("probe")
        .expect("listing");
    assert!(!listing.single_file);
    assert_eq!(listing.root, "/media-bucket/shows");
    let paths: Vec<(&str, bool)> = listing
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.is_dir))
        .collect();
    assert_eq!(
        paths,
        vec![
            ("s1", true),
            ("extra.nfo", false),
            ("s1/e01.mkv", false),
            ("s1/e02.mkv", false)
        ]
    );
    let single = harness
        .service
        .probe(&"s3://media-bucket/shows/extra.nfo".parse().expect("url"))
        .await
        .expect("probe")
        .expect("listing");
    assert!(single.single_file);
    assert_eq!(single.root, "/media-bucket/shows");
    assert!(single.entries[0].etag.is_some());
}

/// The download half of the contract with each provider's profile and link scheme: the
/// connector differs, the runner, the listing and the resume rules do not.
#[tokio::test]
async fn every_provider_meets_the_download_contract() {
    for provider in [
        ObjectStorageProvider::S3,
        ObjectStorageProvider::Azure,
        ObjectStorageProvider::Gcs,
    ] {
        let harness = Harness::start_for(provider).await;
        harness.profile().await;
        let body = payload(400_000);
        harness.put("shows/e01.mkv", body.clone()).await;
        harness.put("shows/e02.mkv", body.clone()).await;

        let listing = harness
            .service
            .probe(&harness.link("shows/"))
            .await
            .expect("probe")
            .unwrap_or_else(|failure| panic!("{provider:?}: {failure:?}"));
        let names: Vec<&str> = listing.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["e01.mkv", "e02.mkv"], "{provider:?}");

        // Resumed behind a quarter recorded against the current version.
        let file = harness.queue("shows/e01.mkv", "e01.mkv").await;
        let version = crate::runner::validator(
            &crate::head(
                &Store {
                    objects: harness.memory.clone(),
                    parts: harness.parts.clone(),
                },
                &Path::from("shows/e01.mkv"),
            )
            .await
            .expect("head"),
        );
        harness.partial(&file, &body[..100_000], version).await;
        let outcome = harness.run(&file).await;
        assert!(
            matches!(outcome, RunOutcome::Completed { .. }),
            "{provider:?}: {outcome:?}"
        );
        let written = tokio::fs::read(harness.destination().join("e01.mkv"))
            .await
            .expect("final");
        assert_eq!(written, body, "{provider:?}");

        // A new version under the same key refuses the resume and keeps the partial file.
        let file = harness.queue("shows/e02.mkv", "e02.mkv").await;
        let stale = etag_of(&harness, "shows/e02.mkv").await;
        harness.partial(&file, &body[..100_000], stale).await;
        harness.put("shows/e02.mkv", body.clone()).await;
        match harness.run(&file).await {
            RunOutcome::Failed(failure) => assert_eq!(
                failure.code.as_deref(),
                Some(crate::error::OBJECT_CHANGED),
                "{provider:?}"
            ),
            other => panic!("{provider:?}: a replaced object was resumed: {other:?}"),
        }
    }
}

/// A build without the `azure` or `gcs` feature keeps the links and the profiles and refuses
/// the transfer with a stable code; a build with them opens the store without a request.
#[tokio::test]
async fn a_provider_left_out_of_the_build_is_refused_with_its_code() {
    for provider in [ObjectStorageProvider::Azure, ObjectStorageProvider::Gcs] {
        let mut harness = Harness::start_for(provider).await;
        harness.service.fixture = None;
        let profile = harness.profile().await;
        let opened = harness
            .service
            .open(&profile, "media-bucket")
            .await
            .expect("open");
        if crate::provider_available(provider) {
            assert!(opened.is_ok(), "{provider:?}");
        } else {
            let failure = opened.err().expect("refused");
            assert_eq!(
                failure.code.as_deref(),
                Some(crate::PROVIDER_UNSUPPORTED),
                "{provider:?}"
            );
        }
    }
}

#[test]
fn the_resume_validator_carries_the_provider_version() {
    let meta = |e_tag: Option<&str>, version: Option<&str>| object_store::ObjectMeta {
        location: Path::from("a"),
        last_modified: chrono::Utc::now(),
        size: 1,
        e_tag: e_tag.map(str::to_owned),
        version: version.map(str::to_owned),
    };
    let validator = crate::runner::validator;
    // Google's generation: the same ETag under a new generation is another object.
    assert_ne!(
        validator(&meta(Some("\"a\""), Some("1700000000000001"))),
        validator(&meta(Some("\"a\""), Some("1700000000000002")))
    );
    // Without a version the ETag alone decides, exactly as before.
    assert_eq!(
        validator(&meta(Some("\"a\""), None)).as_deref(),
        Some("\"a\"")
    );
    assert_eq!(validator(&meta(None, None)), None);
}

#[test]
fn a_secret_is_checked_for_the_shape_its_provider_signs_with() {
    use crate::{ACCOUNT_KEY_INVALID, SAS_INVALID, SERVICE_ACCOUNT_INVALID, secret_problem};
    use ObjectCredentialSource::{SharedAccessSignature, Static};
    use ObjectStorageProvider::{Azure, Gcs, S3};
    assert_eq!(secret_problem(S3, Static, "any secret key"), None);
    assert_eq!(
        secret_problem(Azure, Static, "not base64!"),
        Some(ACCOUNT_KEY_INVALID)
    );
    assert_eq!(secret_problem(Azure, Static, "c2VjcmV0a2V5"), None);
    assert_eq!(
        secret_problem(Azure, SharedAccessSignature, "c2VjcmV0a2V5"),
        Some(SAS_INVALID)
    );
    assert_eq!(
        secret_problem(Azure, SharedAccessSignature, "?sv=2024-11-04&sp=r&sig=x"),
        None
    );
    assert_eq!(
        secret_problem(Gcs, Static, "c2VjcmV0a2V5"),
        Some(SERVICE_ACCOUNT_INVALID)
    );
}

/// A package directory with one file of `length` bytes.
async fn package_file(harness: &Harness, name: &str, length: usize) -> std::path::PathBuf {
    let directory = harness.directory.path().join("finished");
    tokio::fs::create_dir_all(&directory).await.expect("dir");
    tokio::fs::write(directory.join(name), payload(length))
        .await
        .expect("file");
    directory
}

async fn upload(harness: &Harness, directory: &std::path::Path, files: &[String]) -> UploadReport {
    let profile = harness.profile().await;
    harness
        .service
        .upload(
            &profile.id.to_string(),
            ObjectUpload {
                owner: "package-1",
                package_name: "Release: One?",
                directory,
                files,
                destination: "",
                progress: Arc::new(|_, _| {}),
                stop: CancellationToken::new(),
                bandwidth: rd_limits::ScopedLimiter::unlimited(),
            },
        )
        .await
        .expect("upload")
}

const MIB: usize = 1024 * 1024;

#[tokio::test]
async fn a_multipart_upload_continues_after_a_restart_with_the_missing_parts_only() {
    let harness = Harness::start().await;
    let directory = package_file(&harness, "big.bin", 40 * MIB).await;
    let files = vec!["big.bin".to_owned()];
    // Three parts of 16 MiB; the second one fails the first time.
    *lock(&harness.parts.fail_part) = Some(1);
    let first = upload(&harness, &directory, &files).await;
    assert!(matches!(first, UploadReport::Failed { .. }), "{first:?}");
    assert_eq!(*lock(&harness.parts.sent), vec![0]);

    let harness = harness.restart().await;
    let second = upload(&harness, &directory, &files).await;
    assert_eq!(
        second,
        UploadReport::Verified {
            files: files.clone()
        }
    );
    // Part 0 was not sent again.
    assert_eq!(*lock(&harness.parts.sent), vec![0, 1, 2]);
    let stored = harness
        .memory
        .get_opts(
            &Path::from("Release_ One_/big.bin"),
            object_store::GetOptions::default(),
        )
        .await
        .expect("object")
        .bytes()
        .await
        .expect("bytes");
    assert_eq!(stored.as_ref(), payload(40 * MIB).as_slice());
    // Everything verified: nothing is left to continue.
    assert!(
        harness
            .database
            .object_uploads(None, None)
            .await
            .expect("records")
            .is_empty()
    );
}

/// RD-170-16: a file in a folder of the package goes up under its path relative to the
/// package, the folder kept in the key, beside a file at the top.
#[tokio::test]
async fn a_file_in_a_folder_keeps_its_relative_path_in_the_key() {
    let harness = Harness::start().await;
    let directory = package_file(&harness, "top.nfo", 1000).await;
    tokio::fs::create_dir_all(directory.join("Film"))
        .await
        .expect("folder");
    tokio::fs::write(directory.join("Film").join("film.mkv"), payload(2000))
        .await
        .expect("file");
    let files = vec!["Film/film.mkv".to_owned(), "top.nfo".to_owned()];

    let report = upload(&harness, &directory, &files).await;

    assert_eq!(report, UploadReport::Verified { files });
    for (key, length) in [
        ("Release_ One_/Film/film.mkv", 2000),
        ("Release_ One_/top.nfo", 1000),
    ] {
        let stored = harness
            .memory
            .get_opts(&Path::from(key), object_store::GetOptions::default())
            .await
            .expect("object")
            .bytes()
            .await
            .expect("bytes");
        assert_eq!(stored.as_ref(), payload(length).as_slice(), "{key}");
    }
}

/// The part ledger is keyed on the object key, so a nested file continues after a restart
/// exactly as a file at the top does.
#[tokio::test]
async fn a_nested_multipart_upload_continues_after_a_restart() {
    let harness = Harness::start().await;
    let directory = harness.directory.path().join("finished");
    tokio::fs::create_dir_all(directory.join("Film"))
        .await
        .expect("folder");
    tokio::fs::write(directory.join("Film").join("big.bin"), payload(40 * MIB))
        .await
        .expect("file");
    let files = vec!["Film/big.bin".to_owned()];
    *lock(&harness.parts.fail_part) = Some(1);
    let first = upload(&harness, &directory, &files).await;
    assert!(matches!(first, UploadReport::Failed { .. }), "{first:?}");

    let harness = harness.restart().await;
    let second = upload(&harness, &directory, &files).await;

    assert_eq!(second, UploadReport::Verified { files });
    assert_eq!(*lock(&harness.parts.sent), vec![0, 1, 2]);
    let stored = harness
        .memory
        .get_opts(
            &Path::from("Release_ One_/Film/big.bin"),
            object_store::GetOptions::default(),
        )
        .await
        .expect("object")
        .bytes()
        .await
        .expect("bytes");
    assert_eq!(stored.len(), 40 * MIB);
}

#[tokio::test]
async fn a_changed_file_aborts_its_old_upload_and_starts_over() {
    let harness = Harness::start().await;
    let directory = package_file(&harness, "big.bin", 40 * MIB).await;
    let files = vec!["big.bin".to_owned()];
    *lock(&harness.parts.fail_part) = Some(1);
    let _ = upload(&harness, &directory, &files).await;

    // The local file is not the one the recorded parts describe any more.
    package_file(&harness, "big.bin", 36 * MIB).await;
    let report = upload(&harness, &directory, &files).await;
    assert!(
        matches!(report, UploadReport::Verified { .. }),
        "{report:?}"
    );
    assert!(lock(&harness.parts.aborted).contains("mpu-1"));
    let stored = harness
        .memory
        .get_opts(
            &Path::from("Release_ One_/big.bin"),
            object_store::GetOptions::default(),
        )
        .await
        .expect("object")
        .bytes()
        .await
        .expect("bytes");
    assert_eq!(stored.len(), 36 * MIB);
}

#[tokio::test]
async fn a_small_file_goes_up_in_one_request_and_is_verified() {
    let harness = Harness::start().await;
    let directory = package_file(&harness, "small.nfo", 1000).await;
    let files = vec!["small.nfo".to_owned()];
    let report = upload(&harness, &directory, &files).await;
    assert_eq!(report, UploadReport::Verified { files });
    assert!(lock(&harness.parts.sent).is_empty());
}

#[tokio::test]
async fn a_stopped_upload_is_stopped_not_failed() {
    let harness = Harness::start().await;
    let directory = package_file(&harness, "big.bin", 20 * MIB).await;
    let profile = harness.profile().await;
    let stop = CancellationToken::new();
    stop.cancel();
    let report = harness
        .service
        .upload(
            &profile.id.to_string(),
            ObjectUpload {
                owner: "package-1",
                package_name: "p",
                directory: &directory,
                files: &["big.bin".to_owned()],
                destination: "media-bucket/in",
                progress: Arc::new(|_, _| {}),
                stop,
                bandwidth: rd_limits::ScopedLimiter::unlimited(),
            },
        )
        .await
        .expect("upload");
    assert_eq!(report, UploadReport::Stopped);
    // The started upload stays recorded for the next run.
    assert_eq!(
        harness
            .database
            .object_uploads(Some(profile.id), None)
            .await
            .expect("records")
            .len(),
        1
    );
}

#[tokio::test]
async fn the_sweep_aborts_abandoned_uploads() {
    let harness = Harness::start().await;
    let directory = package_file(&harness, "big.bin", 40 * MIB).await;
    *lock(&harness.parts.fail_part) = Some(1);
    let _ = upload(&harness, &directory, &["big.bin".to_owned()]).await;
    // Nothing is older than a week yet.
    assert_eq!(
        harness
            .service
            .sweep_stale_uploads(crate::STALE_UPLOAD_AGE)
            .await
            .expect("sweep"),
        0
    );
    assert_eq!(
        harness
            .service
            .sweep_stale_uploads(std::time::Duration::ZERO)
            .await
            .expect("sweep"),
        1
    );
    assert!(lock(&harness.parts.aborted).contains("mpu-1"));
    assert!(
        harness
            .database
            .object_uploads(None, None)
            .await
            .expect("records")
            .is_empty()
    );
}

#[test]
fn parts_stay_within_the_service_limits() {
    assert_eq!(crate::upload::part_size(1), 16 * MIB as u64);
    // 5 TiB, the largest object S3 stores, in at most 10 000 parts.
    let largest = 5 * 1024 * 1024 * MIB as u64;
    let size = crate::upload::part_size(largest);
    assert!(largest.div_ceil(size) <= 10_000);
    assert_eq!(size % MIB as u64, 0);
}

#[test]
fn a_destination_names_its_bucket_unless_the_profile_is_bound() {
    use crate::upload::split_destination;
    let s3 = ObjectStorageProvider::S3;
    assert_eq!(
        split_destination(s3, "media-bucket/in/new", None),
        Some(("media-bucket", "in/new"))
    );
    assert_eq!(
        split_destination(s3, "", Some("bound-bucket")),
        Some(("bound-bucket", ""))
    );
    assert_eq!(split_destination(s3, "", None), None);
    assert_eq!(split_destination(s3, "Not_A_Bucket/x", None), None);
    // Each provider's own rules: Google takes an underscore, an Azure container no dot.
    assert_eq!(
        split_destination(ObjectStorageProvider::Gcs, "media_bucket/x", None),
        Some(("media_bucket", "x"))
    );
    assert_eq!(
        split_destination(ObjectStorageProvider::Azure, "media.bucket/x", None),
        None
    );
    assert_eq!(
        crate::upload::object_key("in/", "My: Release?", "a\\b.bin"),
        "in/My_ Release_/a/b.bin"
    );
}

/// `object_storage.after_part_upload`: the service confirmed a part the record never heard of.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_part_confirmed_but_not_recorded_is_uploaded_again_and_nothing_before_it() {
    let harness = Harness::start().await;
    let directory = package_file(&harness, "big.bin", 40 * MIB).await;
    let files = vec!["big.bin".to_owned()];
    {
        // Part 0 goes through and is recorded; part 1 is confirmed, then the process stops.
        let guard =
            rd_core::failpoint::FailpointGuard::after("object_storage.after_part_upload", 1);
        let profile = harness.profile().await;
        let crashed = harness
            .service
            .upload(
                &profile.id.to_string(),
                ObjectUpload {
                    owner: "package-1",
                    package_name: "Release: One?",
                    directory: &directory,
                    files: &files,
                    destination: "",
                    progress: Arc::new(|_, _| {}),
                    stop: CancellationToken::new(),
                    bandwidth: rd_limits::ScopedLimiter::unlimited(),
                },
            )
            .await;
        assert!(crashed.is_err(), "{crashed:?}");
        assert!(guard.fired(), "the crash point was never reached");
    }
    assert_eq!(*lock(&harness.parts.sent), vec![0, 1]);

    let harness = harness.restart().await;
    let report = upload(&harness, &directory, &files).await;
    assert_eq!(report, UploadReport::Verified { files });
    // Part 1 again, part 0 not.
    assert_eq!(*lock(&harness.parts.sent), vec![0, 1, 1, 2]);
    let stored = harness
        .memory
        .get_opts(
            &Path::from("Release_ One_/big.bin"),
            object_store::GetOptions::default(),
        )
        .await
        .expect("object")
        .bytes()
        .await
        .expect("bytes");
    assert_eq!(stored.as_ref(), payload(40 * MIB).as_slice());
}

/// The folder the full backup's destinations use (RD-160-02).
mod folder;

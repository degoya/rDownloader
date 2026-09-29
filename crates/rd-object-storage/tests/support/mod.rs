//! What the Azure and Google wire tests share: a service on a fresh database and vault, a
//! queue row for a link, a run of the runner, and a partial file left by an earlier attempt.
//! Each test file brings its own fixture server; `tests/s3.rs` predates this and keeps its own.
//! `tests/s3_tls.rs` and `tests/s3_live.rs` use it for S3 over TLS and against a real service.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use axum::{Router, body::Body, response::Response};
use rd_core::{DownloadKind, DownloadState, PackageId};
use rd_db::{Database, NewDownload, NewObjectStorageProfile, NewPackage};
use rd_object_storage::ObjectStorageService;
use rd_scheduler::{RunLimits, RunOutcome};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn respond(
    status: axum::http::StatusCode,
    headers: &[(&str, String)],
    body: impl Into<Body>,
) -> Response {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    builder
        .body(body.into())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Percent-decodes one URL component; `+` stays a plus, as it does in a path.
pub fn decode(raw: &str) -> String {
    url::form_urlencoded::parse(format!("v={}", raw.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

pub fn payload(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 241) as u8).collect()
}

/// Serves `app` on a free local port and answers its address.
pub async fn serve(app: Router) -> std::net::SocketAddr {
    // A part or block is up to 16 MiB; axum's default 2 MiB body limit would refuse every one.
    let app = app.layer(axum::extract::DefaultBodyLimit::disable());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

pub struct Harness {
    pub directory: tempfile::TempDir,
    pub database: Database,
    pub secrets: rd_secrets::SecretStore,
    pub service: ObjectStorageService,
    pub profile: rd_core::ObjectStorageProfile,
    /// `az` or `gs`: the scheme of the links queued.
    pub scheme: &'static str,
    pub bucket: &'static str,
}

impl Harness {
    /// A service with one profile; `secret` goes into the vault as the profile's secret.
    pub async fn start(
        profile: NewObjectStorageProfile,
        secret: Option<&str>,
        scheme: &'static str,
        bucket: &'static str,
    ) -> Self {
        Self::start_with_network(
            profile,
            secret,
            scheme,
            bucket,
            rd_http::NetworkDefaults::default(),
        )
        .await
    }

    /// [`Harness::start`] under the given network settings — the custom CA of the TLS tests.
    pub async fn start_with_network(
        profile: NewObjectStorageProfile,
        secret: Option<&str>,
        scheme: &'static str,
        bucket: &'static str,
        network: rd_http::NetworkDefaults,
    ) -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("objects.sqlite3"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secrets");
        let secret_ref = match secret {
            Some(value) => Some(secrets.put_string(value.to_owned()).await.expect("secret")),
            None => None,
        };
        let profile = database
            .create_object_storage_profile(NewObjectStorageProfile {
                secret_ref,
                ..profile
            })
            .await
            .expect("profile");
        let service = ObjectStorageService::new(
            database.clone(),
            secrets.clone(),
            Arc::new(RwLock::new(rd_core::RemoteSettings::default())),
            Arc::new(RwLock::new(network)),
        );
        Self {
            directory,
            database,
            secrets,
            service,
            profile,
            scheme,
            bucket,
        }
    }

    pub fn link(&self, key: &str) -> url::Url {
        format!("{}://{}/{key}", self.scheme, self.bucket)
            .parse()
            .expect("url")
    }

    pub fn destination(&self) -> std::path::PathBuf {
        self.directory.path().join("downloads")
    }

    pub async fn queue(
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
            .expect("download");
        (file, package)
    }

    pub async fn run(
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

    /// Leaves `body` behind as the partial file of an earlier attempt that recorded
    /// `validator` and `size` for the object.
    pub async fn partial(
        &self,
        file: &rd_core::DownloadFile,
        body: &[u8],
        size: u64,
        validator: &str,
    ) {
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
            .prepare_transfer(
                file.id,
                Some(size),
                Some(validator.to_owned()),
                None,
                Vec::new(),
            )
            .await
            .expect("validators");
    }

    /// Uploads one file of `body` as the package `release` into the profile's bound bucket.
    pub async fn upload(&self, body: &[u8]) -> rd_extract::UploadReport {
        use rd_extract::{ObjectUpload, ObjectUploader};

        let directory = self.directory.path().join("finished");
        tokio::fs::create_dir_all(&directory).await.expect("dir");
        tokio::fs::write(directory.join("big.bin"), body)
            .await
            .expect("file");
        let files = vec!["big.bin".to_owned()];
        self.service
            .upload(
                &self.profile.id.to_string(),
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
            .expect("upload")
    }
}

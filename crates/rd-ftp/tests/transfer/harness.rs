//! The runner, a database and the fixture server behind it, shared by the transfer cases.

use std::sync::Arc;

use rd_core::{DownloadKind, DownloadState, PackageId, RemoteAuthMode, RemoteProtocol};
use rd_db::{Database, NewDownload, NewPackage, NewRemoteCredential};
use rd_ftp::{FtpRunner, FtpService};
use tokio::sync::RwLock;

use crate::fixture_server::Fixture;

/// Unlimited pacing plus one request slot; the queue supplies these in production.
pub(crate) fn run_limits() -> rd_scheduler::RunLimits {
    rd_scheduler::RunLimits {
        max_parallel_requests: 1,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
        address_policy: None,
    }
}

/// The payload used throughout; large enough to span several read chunks.
pub(crate) fn payload() -> Vec<u8> {
    (0..200_000u32).map(|index| (index % 251) as u8).collect()
}

pub(crate) struct Harness {
    _directory: tempfile::TempDir,
    pub(crate) database: Database,
    pub(crate) service: FtpService,
    pub(crate) fixture: Fixture,
    pub(crate) destination: std::path::PathBuf,
}

impl Harness {
    pub(crate) async fn start() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("ftp.sqlite3"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secrets");
        let fixture = Fixture::start().await;
        let settings = Arc::new(RwLock::new(rd_core::RemoteSettings::default()));
        let service = FtpService::new(
            database.clone(),
            secrets,
            settings,
            Arc::new(RwLock::new(rd_http::NetworkDefaults::default())),
        );
        let destination = directory.path().join("downloads");
        tokio::fs::create_dir_all(&destination).await.expect("dir");
        Self {
            _directory: directory,
            database,
            service,
            fixture,
            destination,
        }
    }

    /// Stores an anonymous login for the fixture and returns its id.
    pub(crate) async fn credential(&self) -> rd_core::RemoteCredentialId {
        self.database
            .create_remote_credential(NewRemoteCredential {
                name: "fixture".to_owned(),
                protocol: RemoteProtocol::Ftp,
                host: "127.0.0.1".to_owned(),
                port: self.fixture.port,
                username: None,
                auth_mode: RemoteAuthMode::Anonymous,
                passive: true,
                enabled: true,
                secret_ref: None,
                key_ref: None,
                passphrase_ref: None,
            })
            .await
            .expect("credential")
            .id
    }

    /// Queues one remote file and returns the row.
    pub(crate) async fn queue(&self, remote_path: &str, name: &str) -> rd_core::DownloadFile {
        let package_id = PackageId::new();
        self.database
            .create_package(NewPackage {
                id: package_id,
                name: "ftp".to_owned(),
                destination: self.destination.to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let source = format!("ftp://127.0.0.1:{}{remote_path}", self.fixture.port);
        self.database
            .create_download(NewDownload {
                id: rd_core::DownloadId::new(),
                package_id,
                source: source.parse().expect("url"),
                file_name: name.to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::Ftp,
                media: None,
                remote_credential_id: Some(self.credential().await),
                mirror_group: None,
                replay: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download")
    }

    pub(crate) fn runner(&self) -> FtpRunner {
        FtpRunner::new(self.service.clone())
    }

    pub(crate) async fn package_of(
        &self,
        file: &rd_core::DownloadFile,
    ) -> rd_core::DownloadPackage {
        self.database
            .list_packages()
            .await
            .expect("packages")
            .into_iter()
            .find(|package| package.id == file.package_id)
            .expect("package")
    }

    pub(crate) fn part_path(&self, file: &rd_core::DownloadFile) -> std::path::PathBuf {
        self.destination
            .join(".rdownloader")
            .join(format!("{}.part", file.id))
    }
}

//! SFTP transfer source (RD-060-02).
//!
//! Provides the scheduler runner for `DownloadKind::Sftp` and the probe the LinkGrabber
//! uses to turn an `sftp://` link into a reviewable directory listing.
//!
//! Bastion/jump-host chaining is deliberately out of scope, as RD-060-02 permits: `russh`
//! can forward a channel, but doing it properly needs a second host-key trust decision per
//! hop, and half of that is worse than none.

mod client;
mod error;
mod hostkey;
mod listing;
mod runner;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use chrono::Utc;
use rd_core::{
    Failure, RemoteCredential, RemoteCredentialId, RemoteListing, RemoteSettings, RemoteTarget,
    SshHostKey,
};
use rd_db::Database;
use rd_secrets::SecretStore;
use tokio::sync::RwLock;

pub use error::{
    AGENT_UNAVAILABLE, AUTH_FAILED, CONNECT_FAILED, FILE_CHANGED, KEY_INVALID, LISTING_TOO_LARGE,
    NO_CREDENTIAL, PATH_NOT_FOUND, UNSAFE_PATH,
};
pub use hostkey::{HOST_KEY_CHANGED, HOST_KEY_UNKNOWN, HOST_KEY_UNSUPPORTED};
pub use runner::SftpRunner;

/// Live remote settings shared between the API handlers and the runner.
pub type SharedRemoteSettings = Arc<RwLock<RemoteSettings>>;

/// What a link turned out to be.
pub enum Probed {
    Resolved(Box<RemoteListing>),
    Failed(Failure),
}

/// Everything the probe and the runner need to reach a server.
#[derive(Clone)]
pub struct SftpService {
    database: Database,
    secrets: SecretStore,
    settings: SharedRemoteSettings,
}

impl SftpService {
    #[must_use]
    pub fn new(database: Database, secrets: SecretStore, settings: SharedRemoteSettings) -> Self {
        Self {
            database,
            secrets,
            settings,
        }
    }

    pub(crate) const fn database(&self) -> &Database {
        &self.database
    }

    pub(crate) fn max_parallel(&self) -> usize {
        self.settings.try_read().map_or(2, |settings| {
            settings.sanitized().remote_max_parallel as usize
        })
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.settings
            .try_read()
            .map_or_else(|_| Duration::from_secs(60), |s| s.sanitized().timeout())
    }

    fn auto_trust(&self) -> bool {
        self.settings
            .try_read()
            .is_ok_and(|settings| settings.remote_ssh_auto_trust)
    }

    /// Opens an authenticated session for a link, recording a first-sighting host key when
    /// `remote_ssh_auto_trust` allowed one through.
    pub(crate) async fn connect(
        &self,
        pinned: Option<RemoteCredentialId>,
        target: &RemoteTarget,
    ) -> Result<Result<client::Connection, Failure>> {
        let credential = match pinned {
            Some(id) => self.database.remote_credential(id).await?,
            None => self.database.match_remote_credential(target).await?,
        };
        let Some(credential) = credential else {
            return Ok(Err(error::no_credential(&target.host)));
        };
        self.connect_with(&credential).await
    }

    /// Opens an authenticated session for one specific stored login.
    pub(crate) async fn connect_with(
        &self,
        credential: &RemoteCredential,
    ) -> Result<Result<client::Connection, Failure>> {
        let password = self.secret(credential.secret_ref.as_deref()).await?;
        let private_key = self.secret(credential.key_ref.as_deref()).await?;
        let passphrase = self.secret(credential.passphrase_ref.as_deref()).await?;
        let database = self.database.clone();
        let host = credential.host.clone();
        let port = credential.port;
        // The trust lookup runs inside the key-exchange callback, which is synchronous from
        // russh's point of view, so the verdict is resolved through a blocking bridge onto
        // the same runtime rather than by holding the store open across the handshake.
        let verdict = Arc::new(move |offered: &hostkey::OfferedKey| {
            let database = database.clone();
            let host = host.clone();
            let algorithm = offered.algorithm.clone();
            let fingerprint = offered.fingerprint.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    database
                        .ssh_host_key_verdict(&host, port, &algorithm, &fingerprint)
                        .await
                        .unwrap_or(rd_db::HostKeyVerdict::Unknown)
                })
            })
        });
        let spec = client::ConnectSpec {
            credential,
            password: password.as_ref(),
            private_key: private_key.as_ref(),
            passphrase: passphrase.as_ref(),
            auto_trust: self.auto_trust(),
            timeout: self.timeout(),
        };
        match client::connect(spec, verdict).await? {
            Ok((connection, offered)) => {
                // Persist what auto-trust accepted, so a later change is still detected.
                self.database
                    .trust_ssh_host_key(SshHostKey {
                        host: credential.host.clone(),
                        port: credential.port,
                        algorithm: offered.algorithm,
                        fingerprint: offered.fingerprint,
                        first_seen: Utc::now(),
                    })
                    .await?;
                Ok(Ok(connection))
            }
            Err(failure) => Ok(Err(failure)),
        }
    }

    async fn secret(&self, reference: Option<&str>) -> Result<Option<secrecy::SecretString>> {
        match reference {
            Some(reference) => Ok(Some(self.secrets.get(reference).await?)),
            None => Ok(None),
        }
    }

    /// Resolves one link into the listing the LinkGrabber reviews.
    pub async fn probe(
        &self,
        target: &RemoteTarget,
        pinned: Option<RemoteCredentialId>,
    ) -> Result<(Probed, Option<RemoteCredentialId>)> {
        let credential = match pinned {
            Some(id) => self.database.remote_credential(id).await?,
            None => self.database.match_remote_credential(target).await?,
        };
        let Some(credential) = credential else {
            return Ok((Probed::Failed(error::no_credential(&target.host)), None));
        };
        let connection = match self.connect_with(&credential).await? {
            Ok(connection) => connection,
            Err(failure) => return Ok((Probed::Failed(failure), None)),
        };
        let probed = self.probe_on(&connection.sftp, target).await?;
        Ok((probed, Some(credential.id)))
    }

    async fn probe_on(
        &self,
        sftp: &russh_sftp::client::SftpSession,
        target: &RemoteTarget,
    ) -> Result<Probed> {
        let path = normalize(&target.path);
        let metadata = match sftp.metadata(path.clone()).await {
            Ok(metadata) => metadata,
            Err(error) => return Ok(Probed::Failed(error::classify_sftp(&error))),
        };
        if metadata.is_dir() {
            // Resolve the path the server actually means, so a relative or symlinked
            // directory is walked under its real name.
            let root = sftp.canonicalize(path.clone()).await.unwrap_or(path);
            return Ok(Probed::Resolved(Box::new(
                listing::walk(sftp, &root).await?,
            )));
        }
        let name = file_name(&path);
        if !rd_core::is_safe_relative_path(&name) {
            return Ok(Probed::Failed(Failure::coded(
                rd_core::FailureKind::Permanent,
                error::UNSAFE_PATH,
                "The remote file name cannot be stored safely",
            )));
        }
        Ok(Probed::Resolved(Box::new(RemoteListing {
            root: parent(&path),
            single_file: true,
            entries: vec![rd_core::RemoteEntry {
                path: name,
                is_dir: false,
                size: listing::size_of(&metadata),
                modified: listing::modified_at(&metadata),
                etag: None,
            }],
            truncated: None,
            supports_resume: true,
        })))
    }

    /// Checks that a stored login can reach its server and log in, for the settings UI.
    pub async fn test_credential(&self, credential: &RemoteCredential) -> Result<Option<Failure>> {
        match self.connect_with(credential).await? {
            Ok(_) => Ok(None),
            Err(failure) => Ok(Some(failure)),
        }
    }
}

fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    trimmed.to_owned()
}

fn file_name(path: &str) -> String {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => path[..index].to_owned(),
    }
}

/// Builds the runner registered with the scheduler.
#[must_use]
pub fn build(service: SftpService) -> Arc<dyn rd_scheduler::ExternalRunner> {
    Arc::new(SftpRunner::new(service))
}

#[cfg(test)]
mod tests {
    use super::{file_name, normalize, parent};

    #[test]
    fn paths_normalize_to_a_server_absolute_form() {
        assert_eq!(normalize("/srv/a.bin"), "/srv/a.bin");
        assert_eq!(normalize("/srv/"), "/srv");
        assert_eq!(normalize(""), "/");
    }

    #[test]
    fn file_and_parent_split_the_path() {
        assert_eq!(file_name("/srv/dir/a.bin"), "a.bin");
        assert_eq!(parent("/srv/dir/a.bin"), "/srv/dir");
        assert_eq!(parent("/a.bin"), "/");
    }
}

//! Object storage as a download source and an upload target (RD-150-04, RD-150-05).
//!
//! S3 and every service that speaks its API, Azure Blob Storage and Google Cloud Storage,
//! through the `object_store` crate of the Apache Arrow project: its request signing, its
//! multipart API and its credential providers (environment, web and workload identity,
//! container and instance metadata, token refresh) rather than a copy of them. The
//! provider-specific part is [`connect`], with Azure and Google behind the `azure` and `gcs`
//! features; listing, the resumable download and the recoverable multipart upload talk to the
//! provider-neutral traits.
//!
//! A link names a bucket and a key, never a host: the endpoint and the credentials come from
//! a profile somebody configured, chosen by `rd_core::select_profile`.
//!
//! [`ObjectFolder`] is the same machinery as a plain file store for the full backup's
//! destinations (RD-160-02): put, list, get and delete of single files in one folder.

mod connect;
pub mod error;
mod folder;
mod listing;
mod runner;
#[cfg(test)]
mod tests;
mod upload;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use object_store::{GetOptions, path::Path};
use rd_core::{
    ByteCount, Failure, FailureKind, ObjectAddress, ObjectCredentialSource, ObjectStorageProfile,
    ObjectStorageProvider, RemoteEntry, RemoteListing, RemoteSettings,
};
use rd_db::Database;
use rd_secrets::SecretStore;
use secrecy::SecretString;
use tokio::sync::RwLock;

pub use folder::{
    FOLDER_BUCKET_INVALID, FOLDER_NAME_INVALID, FOLDER_PROFILE_MISSING, FolderObject, ObjectFolder,
};
pub use runner::ObjectStorageRunner;
pub use upload::STALE_UPLOAD_AGE;

use connect::{OpenError, Opening, Store};

/// Live remote settings shared with the FTP and SFTP runners (`rd_ftp::SharedRemoteSettings`).
pub type SharedRemoteSettings = Arc<RwLock<RemoteSettings>>;

/// Stable code of a profile whose endpoint is not a usable `http(s)` URL.
pub const ENDPOINT_INVALID: &str = "object_storage.endpoint_invalid";
/// Stable code of a connection test on a profile that names no bucket to test against.
pub const TEST_NEEDS_BUCKET: &str = "object_storage.test_needs_bucket";
/// Stable code of a provider whose connector this build was made without.
pub const PROVIDER_UNSUPPORTED: &str = "object_storage.provider_unsupported";
/// Stable code of an Azure account key that is not the base64 the portal hands out.
pub const ACCOUNT_KEY_INVALID: &str = "object_storage.account_key_invalid";
/// Stable code of an Azure shared access signature without its version or signature.
pub const SAS_INVALID: &str = "object_storage.sas_invalid";
/// Stable code of a Google service account key that is not the console's JSON key file.
pub const SERVICE_ACCOUNT_INVALID: &str = "object_storage.service_account_invalid";

/// Why a secret typed on the settings page cannot sign for its provider, as a stable code.
///
/// Only the shape is checked — base64, the SAS fields, the JSON key's fields — so a typo is
/// named when the profile is saved rather than at its first transfer. Whether the service
/// accepts it is what the connection test is for.
#[must_use]
pub fn secret_problem(
    provider: ObjectStorageProvider,
    source: ObjectCredentialSource,
    secret: &str,
) -> Option<&'static str> {
    match (provider, source) {
        (ObjectStorageProvider::Azure, ObjectCredentialSource::Static)
            if !connect::azure::is_account_key(secret) =>
        {
            Some(ACCOUNT_KEY_INVALID)
        }
        (ObjectStorageProvider::Azure, ObjectCredentialSource::SharedAccessSignature)
            if connect::azure::sas_query(secret).is_none() =>
        {
            Some(SAS_INVALID)
        }
        (ObjectStorageProvider::Gcs, ObjectCredentialSource::Static)
            if !connect::gcs::is_service_account_key(secret) =>
        {
            Some(SERVICE_ACCOUNT_INVALID)
        }
        _ => None,
    }
}

/// Whether this build carries the connector for `provider`.
#[must_use]
pub const fn provider_available(provider: ObjectStorageProvider) -> bool {
    match provider {
        ObjectStorageProvider::S3 => true,
        ObjectStorageProvider::Azure => cfg!(feature = "azure"),
        ObjectStorageProvider::Gcs => cfg!(feature = "gcs"),
    }
}

/// Everything the probe, the runner and the uploader need to reach a bucket.
#[derive(Clone)]
pub struct ObjectStorageService {
    database: Database,
    secrets: SecretStore,
    settings: SharedRemoteSettings,
    network: rd_http::SharedNetworkDefaults,
    /// Replaces [`connect::open`] in the crate's own tests, which run against memory.
    #[cfg(test)]
    fixture: Option<Store>,
}

impl ObjectStorageService {
    #[must_use]
    pub fn new(
        database: Database,
        secrets: SecretStore,
        settings: SharedRemoteSettings,
        network: rd_http::SharedNetworkDefaults,
    ) -> Self {
        Self {
            database,
            secrets,
            settings,
            network,
            #[cfg(test)]
            fixture: None,
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

    /// The profile that serves an address.
    pub async fn resolve(
        &self,
        address: &ObjectAddress,
    ) -> Result<Result<ObjectStorageProfile, Failure>> {
        let profiles = self.database.list_object_storage_profiles().await?;
        Ok(rd_core::select_profile(&profiles, address)
            .cloned()
            .map_err(|choice| {
                error::no_profile(choice, &address.bucket, address.profile.as_deref())
            }))
    }

    /// Opens a store for one bucket of a profile, with its secrets read from the vault.
    pub(crate) async fn open(
        &self,
        profile: &ObjectStorageProfile,
        bucket: &str,
    ) -> Result<Result<Store, Failure>> {
        #[cfg(test)]
        if let Some(store) = &self.fixture {
            return Ok(Ok(store.clone()));
        }
        let secret = self.secret(profile.secret_ref.as_deref()).await?;
        let session_token = self.secret(profile.session_token_ref.as_deref()).await?;
        let (proxy, custom_ca_pem) = self.network_settings().await?;
        let opened = connect::open(Opening {
            profile,
            bucket,
            secret,
            session_token,
            proxy,
            custom_ca_pem: &custom_ca_pem,
            timeout: self.timeout(),
        });
        Ok(opened.map_err(|error| match error {
            OpenError::Endpoint => Failure::coded(
                FailureKind::Permanent,
                ENDPOINT_INVALID,
                "The profile's endpoint is not a usable address",
            ),
            OpenError::Certificate => Failure::coded(
                FailureKind::Permanent,
                error::CONNECT_FAILED,
                "The custom CA certificate could not be read",
            ),
            OpenError::Credentials => Failure::coded(
                FailureKind::AuthRequired,
                error::AUTH_FAILED,
                "The profile's credentials are incomplete or cannot be read",
            ),
            OpenError::Unsupported => Failure::coded(
                FailureKind::Unsupported,
                PROVIDER_UNSUPPORTED,
                "This build has no connector for the profile's provider",
            )
            .with_param("provider", profile.provider.as_str()),
            OpenError::Other => Failure::coded(
                FailureKind::Permanent,
                error::REQUEST_FAILED,
                "The object storage profile could not be used",
            ),
        }))
    }

    async fn secret(&self, reference: Option<&str>) -> Result<Option<SecretString>> {
        match reference {
            Some(reference) => Ok(Some(self.secrets.get(reference).await?)),
            None => Ok(None),
        }
    }

    /// The global proxy (as a URL with its credentials) and the custom CA — the same two the
    /// HTTP engine, FTPS and Usenet use, so an operator's network rules reach this too.
    async fn network_settings(&self) -> Result<(Option<SecretString>, Vec<Vec<u8>>)> {
        let (proxy_id, custom_ca_pem) = {
            let network = self.network.read().await;
            (
                network.global_proxy_profile_id,
                network.custom_ca_pem.clone(),
            )
        };
        let Some(proxy_id) = proxy_id else {
            return Ok((None, custom_ca_pem));
        };
        let Some(proxy) = self.database.proxy_profile(proxy_id).await? else {
            return Ok((None, custom_ca_pem));
        };
        let mut url = proxy.endpoint.clone();
        if let Some(username) = proxy.username.as_deref() {
            let _ = url.set_username(username);
            if let Some(reference) = proxy.secret_ref.as_deref() {
                let password = self.secrets.get(reference).await?;
                let _ = url.set_password(Some(secrecy::ExposeSecret::expose_secret(&password)));
            }
        }
        Ok((Some(SecretString::from(url.to_string())), custom_ca_pem))
    }

    /// Resolves one link into the listing the LinkGrabber reviews.
    ///
    /// A key that names no object is tried as a prefix before it is reported missing: in a
    /// bucket `shows` and `shows/` are different things, and people paste the first meaning
    /// the second.
    pub async fn probe(&self, url: &url::Url) -> Result<Result<RemoteListing, Failure>> {
        let Some(address) = ObjectAddress::parse(url) else {
            return Ok(Err(error::address_invalid()));
        };
        let profile = match self.resolve(&address).await? {
            Ok(profile) => profile,
            Err(failure) => return Ok(Err(failure)),
        };
        let store = match self.open(&profile, &address.bucket).await? {
            Ok(store) => store,
            Err(failure) => return Ok(Err(failure)),
        };
        if address.is_prefix() {
            return Ok(listing::walk(&store, &address).await);
        }
        let Ok(location) = Path::parse(&address.key) else {
            return Ok(Err(error::address_invalid()));
        };
        match head(&store, &location).await {
            Ok(meta) => {
                let Some(name) = address.file_name() else {
                    return Ok(Err(error::address_invalid()));
                };
                if !rd_core::is_safe_relative_path(&name) {
                    return Ok(Err(error::unsafe_path()));
                }
                let parent = address
                    .key
                    .rsplit_once('/')
                    .map_or("", |(parent, _)| parent);
                Ok(Ok(RemoteListing {
                    root: format!("/{}/{parent}", address.bucket)
                        .trim_end_matches('/')
                        .to_owned(),
                    single_file: true,
                    entries: vec![RemoteEntry {
                        path: name,
                        is_dir: false,
                        size: ByteCount::new(meta.size).ok(),
                        modified: Some(meta.last_modified),
                        etag: meta.e_tag,
                    }],
                    truncated: None,
                    supports_resume: true,
                }))
            }
            Err(object_store::Error::NotFound { .. }) => {
                let folder = address.child("");
                match listing::walk(&store, &folder).await {
                    Ok(listing) if !listing.entries.is_empty() => Ok(Ok(listing)),
                    Ok(_) => Ok(Err(error::classify(
                        &object_store::Error::NotFound {
                            path: String::new(),
                            source: "no object and no prefix".into(),
                        },
                        &address.bucket,
                    ))),
                    Err(failure) => Ok(Err(failure)),
                }
            }
            Err(other) => Ok(Err(error::classify(&other, &address.bucket))),
        }
    }

    /// Checks that a profile reaches its bucket and signs acceptably, for the settings page.
    ///
    /// Lists one page of the bound bucket, the cheapest request that needs both. A profile
    /// bound to no bucket has nothing to test against; saying so beats testing a bucket
    /// somebody else owns.
    pub async fn test_profile(&self, profile: &ObjectStorageProfile) -> Result<Option<Failure>> {
        let Some(bucket) = profile.bucket.as_deref() else {
            return Ok(Some(Failure::coded(
                FailureKind::Permanent,
                TEST_NEEDS_BUCKET,
                "Bind the profile to a bucket to test it",
            )));
        };
        let store = match self.open(profile, bucket).await? {
            Ok(store) => store,
            Err(failure) => return Ok(Some(failure)),
        };
        Ok(store
            .objects
            .list_with_delimiter(None)
            .await
            .err()
            .map(|error| error::classify(&error, bucket)))
    }
}

/// `HEAD` through the one trait method every store implements.
pub(crate) async fn head(
    store: &Store,
    location: &Path,
) -> Result<object_store::ObjectMeta, object_store::Error> {
    let options = GetOptions {
        head: true,
        ..GetOptions::default()
    };
    Ok(store.objects.get_opts(location, options).await?.meta)
}

/// Builds the runner registered with the scheduler.
#[must_use]
pub fn build(service: ObjectStorageService) -> Arc<dyn rd_scheduler::ExternalRunner> {
    Arc::new(ObjectStorageRunner::new(service))
}

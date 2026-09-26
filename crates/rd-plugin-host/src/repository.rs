//! Signed plugin repositories (RD-140-01): fetch, cache, refresh, offers and updates.
//!
//! Follows the tool manifest's service (`rd_tools::ManagedToolService`): an index is fetched
//! over https, verified against the repository's key and its replay floor, the floor is
//! persisted *before* the index is adopted, and the verified bytes are cached. At start every
//! cached index is verified again — signature, floor, expiry — and one that fails is deleted
//! rather than trusted, so an installation without network offers only what still verifies.
//!
//! **A repository delivers; it vouches for nothing.** A package offered here installs through
//! `PluginInstaller::install_bytes` like an uploaded one, under a trusted *plugin* key, after
//! its bytes have matched the index's digest. What a repository may withdraw is narrower still:
//! the official repository withdraws package digests and plugin signing keys; a third-party one
//! withdraws only package versions that were installed here from it, and no keys at all,
//! because a repository somebody added for one plugin must not be able to switch off another.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Context;
use chrono::Utc;
use rd_db::{Database, OFFICIAL_REPOSITORY_ID, PluginRepository, RepositoryCheck};
use rd_sign::{Role, TrustStore};
use serde::{Deserialize, Serialize};

use crate::{
    PluginInstaller,
    index::{self, IndexError, IndexPackage, PluginIndex},
};

#[path = "repository_apply.rs"]
mod apply;
#[path = "repository_fetch.rs"]
mod fetch;
#[path = "repository_offers.rs"]
mod offers;

pub use apply::{WithdrawalScope, withdrawal_scope};
pub use fetch::{FETCH_TIMEOUT_SECONDS, Fetcher, HttpFetcher};
pub use offers::{Offer, PackageCompatibility, Update, compatibility};

/// Where the official index is published: the newest release's asset, so the address never
/// changes and the newest index is always the one found there (`docs/plugins.md`).
pub const OFFICIAL_INDEX_URL: &str =
    "https://github.com/degoya/rDownloader/releases/latest/download/rdownloader-plugin-index.json";

/// Hours between two automatic refreshes when nobody chose otherwise.
pub const DEFAULT_REFRESH_HOURS: u32 = 24;
/// Bounds of the refresh interval, in hours: at most hourly, at least weekly.
pub const REFRESH_HOURS_RANGE: std::ops::RangeInclusive<u32> = 1..=168;
/// How long after start the first refresh waits, so it never competes with the start itself.
pub const STARTUP_DELAY: Duration = Duration::from_secs(120);
/// The settings key the interval is stored under.
pub const SETTINGS_KEY: &str = "plugin_repositories";

/// Whether a plugin's newer version installs itself. Stored per plugin by the version manager
/// (RD-140-02); this module only asks.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePolicy {
    /// Shown and installed on a click. The default for every plugin (owner, 2026-09-26).
    #[default]
    Manual,
    /// Installed by the refresh that finds it; still active only after a restart.
    Automatic,
}

/// Where the update policy of a plugin comes from. The version manager implements this over its
/// own store and hands it over with [`PluginRepositoryService::set_update_policy`].
#[async_trait::async_trait]
pub trait UpdatePolicySource: Send + Sync {
    async fn policy(&self, plugin_id: &str) -> UpdatePolicy;
}

/// Every plugin manual: the policy until something else is set.
pub struct ManualUpdates;

#[async_trait::async_trait]
impl UpdatePolicySource for ManualUpdates {
    async fn policy(&self, _plugin_id: &str) -> UpdatePolicy {
        UpdatePolicy::Manual
    }
}

fn manual_updates() -> Arc<dyn UpdatePolicySource> {
    Arc::new(ManualUpdates)
}

/// Why a repository operation failed. [`code`](Self::code) is what the interface translates.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("the plugin repository does not exist")]
    NotFound,
    #[error("the plugin repository is switched off")]
    Disabled,
    #[error("no enabled plugin repository offers {id} {version}")]
    NotOffered { id: String, version: String },
    #[error("the repository address is not a plain https:// URL")]
    InvalidUrl,
    #[error("the repository key is not a Base64 Ed25519 public key")]
    InvalidKey,
    #[error("the pasted key does not sign this repository's index")]
    KeyDoesNotSign,
    #[error("this repository is already added")]
    AlreadyAdded,
    #[error("this build carries no key for the official plugin repository")]
    OfficialKeyMissing,
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("the download failed: {0}")]
    Download(String),
    #[error("the downloaded package is not the one the index describes")]
    DigestMismatch,
    /// The bytes are the ones the index named, but the entry says something else about them:
    /// another publisher, other permissions, another plugin — or no signature at all.
    #[error("the downloaded package does not match its index entry: {0}")]
    NotAsDescribed(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl RepositoryError {
    /// The stable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "plugin_repository.not_found",
            Self::Disabled => "plugin_repository.disabled",
            Self::NotOffered { .. } => "plugin_repository.not_offered",
            Self::InvalidUrl => "plugin_repository.url_invalid",
            Self::InvalidKey => "plugin_repository.key_invalid",
            Self::KeyDoesNotSign => "plugin_repository.key_does_not_sign",
            Self::AlreadyAdded => "plugin_repository.already_added",
            Self::OfficialKeyMissing => "plugin_repository.official_key_missing",
            Self::Index(error) => error.code(),
            Self::Download(_) => "plugin_repository.download_failed",
            Self::DigestMismatch => "plugin_repository.digest_mismatch",
            Self::NotAsDescribed(_) => "plugin_repository.package_mismatch",
            Self::Other(_) => "plugin_repository.failed",
        }
    }
}

/// One verified index held in memory, with the address its relative package URLs resolve
/// against.
#[derive(Clone, Debug)]
pub(crate) struct LoadedIndex {
    pub url: url::Url,
    pub index: PluginIndex,
}

/// What a probe of a third-party repository found: enough to ask the person to approve its key.
#[derive(Clone, Debug)]
pub struct RepositoryProbe {
    pub url: url::Url,
    pub key_id: String,
    /// The pasted key, re-encoded in the one canonical Base64 spelling.
    pub public_key: String,
    pub fingerprint: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) index: PluginIndex,
}

impl RepositoryProbe {
    /// How many packages the index offers, for the approval dialog.
    #[must_use]
    pub fn package_count(&self) -> usize {
        self.index.packages.len()
    }
}

struct Inner {
    database: Database,
    root: PathBuf,
    installer: PluginInstaller,
    fetcher: Arc<dyn Fetcher>,
    /// The official repository's trust; `None` means the compiled-in `Role::Repository` root.
    official_trust: Option<TrustStore>,
    indexes: RwLock<HashMap<String, LoadedIndex>>,
    policy: RwLock<Arc<dyn UpdatePolicySource>>,
    /// One refresh at a time: two would race the floor and the cache file of one repository.
    refreshing: tokio::sync::Mutex<()>,
}

/// Repositories, their verified indexes, and the offers and updates read from them.
#[derive(Clone)]
pub struct PluginRepositoryService(Arc<Inner>);

impl PluginRepositoryService {
    /// A service over `root` (`<data>/plugin-repositories`) that fetches over https.
    #[must_use]
    pub fn new(database: Database, root: PathBuf, installer: PluginInstaller) -> Self {
        Self::with_fetcher(
            database,
            root,
            installer,
            Arc::new(HttpFetcher::new()),
            None,
        )
    }

    /// The same service with its fetcher and the official repository's trust replaced, for the
    /// tests that serve an index without a TLS server or the release key.
    #[doc(hidden)]
    #[must_use]
    pub fn with_fetcher(
        database: Database,
        root: PathBuf,
        installer: PluginInstaller,
        fetcher: Arc<dyn Fetcher>,
        official_trust: Option<TrustStore>,
    ) -> Self {
        Self(Arc::new(Inner {
            database,
            root,
            installer,
            fetcher,
            official_trust,
            indexes: RwLock::new(HashMap::new()),
            policy: RwLock::new(manual_updates()),
            refreshing: tokio::sync::Mutex::new(()),
        }))
    }

    /// Replaces where the per-plugin update policy is read from.
    pub fn set_update_policy(&self, source: Arc<dyn UpdatePolicySource>) {
        if let Ok(mut policy) = self.0.policy.write() {
            *policy = source;
        }
    }

    /// The update policy of one plugin.
    pub async fn policy(&self, plugin_id: &str) -> UpdatePolicy {
        let source = self
            .0
            .policy
            .read()
            .map(|policy| policy.clone())
            .unwrap_or_else(|_| manual_updates());
        source.policy(plugin_id).await
    }

    /// The installer packages from a repository go through.
    #[must_use]
    pub fn installer(&self) -> &PluginInstaller {
        &self.0.installer
    }

    pub(crate) fn database(&self) -> &Database {
        &self.0.database
    }

    /// Clears leftover downloads and loads every cached index that still verifies.
    ///
    /// Called once at start. Never fails: a cache that does not verify is deleted and its
    /// repository offers nothing until the next refresh, which is the offline rule.
    pub async fn load(&self) {
        let _ = tokio::fs::remove_dir_all(self.downloads()).await;
        let repositories = match self.0.database.list_plugin_repositories().await {
            Ok(repositories) => repositories,
            Err(error) => {
                tracing::warn!(%error, "could not read the plugin repositories");
                return;
            }
        };
        for repository in repositories {
            let path = self.cache_path(&repository.id);
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            match self.verify_cached(&repository, &bytes) {
                Ok(loaded) => self.adopt(&repository.id, loaded),
                Err(error) => {
                    tracing::warn!(
                        repository = %repository.id,
                        code = error.code(),
                        %error,
                        "cached plugin index does not verify; deleting it"
                    );
                    let _ = tokio::fs::remove_file(&path).await;
                }
            }
        }
    }

    /// Refreshes every enabled repository and returns each one's outcome, in list order.
    ///
    /// A failure of one repository is recorded on its row and does not stop the others.
    pub async fn refresh_all(&self) -> Vec<(String, Result<(), RepositoryError>)> {
        let _guard = self.0.refreshing.lock().await;
        let repositories = match self.0.database.list_plugin_repositories().await {
            Ok(repositories) => repositories,
            Err(error) => return vec![(String::new(), Err(error.into()))],
        };
        let mut outcomes = Vec::new();
        for repository in repositories
            .into_iter()
            .filter(|repository| repository.enabled)
        {
            let outcome = self.refresh_one(&repository).await;
            if let Err(error) = &outcome {
                tracing::warn!(repository = %repository.id, code = error.code(), %error, "plugin repository refresh failed");
                let _ = self
                    .0
                    .database
                    .record_plugin_repository_check(
                        repository.id.clone(),
                        RepositoryCheck::Failed {
                            code: error.code().to_owned(),
                        },
                    )
                    .await;
            }
            outcomes.push((repository.id, outcome));
        }
        outcomes
    }

    async fn refresh_one(&self, repository: &PluginRepository) -> Result<(), RepositoryError> {
        // A build without the repository key cannot verify anything the official address
        // serves, so it does not ask: no request leaves for an answer that is refused anyway.
        if repository.is_official()
            && self.0.official_trust.is_none()
            && rd_sign::keys_for(Role::Repository, Utc::now()).is_empty()
        {
            return Err(RepositoryError::OfficialKeyMissing);
        }
        let url = repository_url(repository)?;
        let bytes = self.fetch(&url, index::MAX_INDEX_BYTES as u64).await?;
        let trust = self.trust(repository)?;
        let cached = tokio::fs::read(self.cache_path(&repository.id)).await.ok();
        // The same bytes again is the ordinary case — the official index changes with a release,
        // not with every refresh — and is not a replay: it is the index already accepted, so it
        // is checked as the cache is at start, for its signature and its expiry.
        let known = if cached.as_deref() == Some(bytes.as_slice()) {
            None
        } else {
            repository
                .sequence
                .and_then(|sequence| u64::try_from(sequence).ok())
        };
        let index = index::verify_with(&bytes, &trust, known, Utc::now())?;
        self.accept(repository, &url, bytes, index).await
    }

    /// Persists the floor, caches the bytes, applies the withdrawals and adopts the index.
    pub(crate) async fn accept(
        &self,
        repository: &PluginRepository,
        url: &url::Url,
        bytes: Vec<u8>,
        index: PluginIndex,
    ) -> Result<(), RepositoryError> {
        // The floor first: a crash after it and before the cache leaves the floor raised and the
        // old cache refused at the next start, never an older index accepted over a newer one.
        self.0
            .database
            .record_plugin_repository_check(
                repository.id.clone(),
                RepositoryCheck::Accepted {
                    sequence: i64::try_from(index.sequence).unwrap_or(i64::MAX),
                    issued_at: index.issued_at.to_rfc3339(),
                },
            )
            .await?;
        self.write_cache(&repository.id, &bytes).await?;
        self.apply_withdrawals(repository, &index).await;
        self.adopt(
            &repository.id,
            LoadedIndex {
                url: url.clone(),
                index,
            },
        );
        Ok(())
    }

    /// Fetches a third-party index and checks that `public_key` signs it.
    pub async fn probe(
        &self,
        url: &str,
        public_key: &str,
    ) -> Result<RepositoryProbe, RepositoryError> {
        let url = parse_https(url)?;
        let key = rd_sign::decode_public_key(public_key.trim())
            .map_err(|_| RepositoryError::InvalidKey)?;
        let bytes = self.fetch(&url, index::MAX_INDEX_BYTES as u64).await?;
        // The envelope names its key only by id, never the key itself; each id it names is
        // tried with the pasted key, so the approval binds the id the index actually uses.
        for key_id in signature_key_ids(&bytes) {
            let trust = TrustStore::new();
            trust.trust(key_id.clone(), key)?;
            match index::verify_with(&bytes, &trust, None, Utc::now()) {
                Ok(index) => {
                    return Ok(RepositoryProbe {
                        url,
                        key_id,
                        public_key: base64_key(&key),
                        fingerprint: rd_sign::key_fingerprint(&key),
                        bytes,
                        index,
                    });
                }
                Err(IndexError::BadSignature(_) | IndexError::Untrusted(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(RepositoryError::KeyDoesNotSign)
    }

    /// Records a probed repository whose key the person approved, and adopts its index.
    pub async fn add(
        &self,
        name: String,
        probe: RepositoryProbe,
    ) -> Result<PluginRepository, RepositoryError> {
        let existing = self.0.database.list_plugin_repositories().await?;
        if existing
            .iter()
            .any(|repository| repository.url.as_deref() == Some(probe.url.as_str()))
        {
            return Err(RepositoryError::AlreadyAdded);
        }
        let repository = self
            .0
            .database
            .add_plugin_repository(rd_db::NewPluginRepository {
                id: uuid::Uuid::now_v7().to_string(),
                name,
                url: probe.url.to_string(),
                key_id: probe.key_id,
                public_key: probe.public_key,
                fingerprint: probe.fingerprint,
            })
            .await?;
        self.accept(&repository, &probe.url, probe.bytes, probe.index)
            .await?;
        Ok(repository)
    }

    /// Removes a third-party repository, its cache and its offers. What was installed from it
    /// stays installed.
    pub async fn remove(&self, id: &str) -> Result<bool, RepositoryError> {
        let removed = self
            .0
            .database
            .delete_plugin_repository(id.to_owned())
            .await?;
        if removed {
            let _ = tokio::fs::remove_file(self.cache_path(id)).await;
            if let Ok(mut indexes) = self.0.indexes.write() {
                indexes.remove(id);
            }
        }
        Ok(removed)
    }

    /// The refresh interval in hours, from the settings or the default.
    pub async fn refresh_hours(&self) -> u32 {
        self.0
            .database
            .get_setting(SETTINGS_KEY)
            .await
            .ok()
            .flatten()
            .and_then(|value| value.get("refresh_hours")?.as_u64())
            .and_then(|hours| u32::try_from(hours).ok())
            .filter(|hours| REFRESH_HOURS_RANGE.contains(hours))
            .unwrap_or(DEFAULT_REFRESH_HOURS)
    }

    /// Stores the refresh interval; the caller has checked the range.
    pub async fn set_refresh_hours(&self, hours: u32) -> anyhow::Result<()> {
        self.0
            .database
            .set_setting(
                SETTINGS_KEY.to_owned(),
                serde_json::json!({ "refresh_hours": hours }),
            )
            .await
    }

    /// When the loaded index of `id` expires, if one is loaded.
    #[must_use]
    pub fn index_expiry(&self, id: &str) -> Option<chrono::DateTime<Utc>> {
        self.0
            .indexes
            .read()
            .ok()?
            .get(id)
            .map(|loaded| loaded.index.not_after)
    }

    pub(crate) fn loaded(&self) -> BTreeMap<String, LoadedIndex> {
        self.0
            .indexes
            .read()
            .map(|indexes| {
                indexes
                    .iter()
                    .map(|(id, loaded)| (id.clone(), loaded.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) async fn fetch(
        &self,
        url: &url::Url,
        limit: u64,
    ) -> Result<Vec<u8>, RepositoryError> {
        tokio::time::timeout(
            Duration::from_secs(FETCH_TIMEOUT_SECONDS),
            self.0.fetcher.fetch(url, limit),
        )
        .await
        .map_err(|_| {
            RepositoryError::Download(format!("no answer within {FETCH_TIMEOUT_SECONDS} seconds"))
        })?
        .map_err(|error| RepositoryError::Download(format!("{error:#}")))
    }

    fn trust(&self, repository: &PluginRepository) -> Result<TrustStore, RepositoryError> {
        if repository.is_official() {
            return match &self.0.official_trust {
                Some(trust) => Ok(trust.clone()),
                None => rd_sign::trust_store_for(Role::Repository, Utc::now()).map_err(|error| {
                    RepositoryError::Index(IndexError::Untrusted(error.to_string()))
                }),
            };
        }
        let (Some(key_id), Some(public_key)) = (&repository.key_id, &repository.public_key) else {
            return Err(RepositoryError::InvalidKey);
        };
        let trust = TrustStore::new();
        trust
            .trust_base64(key_id.clone(), public_key)
            .map_err(|_| RepositoryError::InvalidKey)?;
        Ok(trust)
    }

    /// Verifies a cached index as the start does: signature and expiry, and not below the floor.
    fn verify_cached(
        &self,
        repository: &PluginRepository,
        bytes: &[u8],
    ) -> Result<LoadedIndex, RepositoryError> {
        let url = repository_url(repository)?;
        // No known sequence, as in the tool manifest's cache: this document *is* the floor, and
        // passing the floor in would make the cache a replay of itself.
        let index = index::verify_with(bytes, &self.trust(repository)?, None, Utc::now())?;
        if let Some(floor) = repository.sequence
            && i64::try_from(index.sequence).unwrap_or(i64::MAX) < floor
        {
            return Err(IndexError::Stale(rd_sign::StaleError::Replayed {
                saw: index.sequence,
                known: u64::try_from(floor).unwrap_or_default(),
            })
            .into());
        }
        Ok(LoadedIndex { url, index })
    }

    fn adopt(&self, id: &str, loaded: LoadedIndex) {
        if let Ok(mut indexes) = self.0.indexes.write() {
            indexes.insert(id.to_owned(), loaded);
        }
    }

    async fn write_cache(&self, id: &str, bytes: &[u8]) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.0.root).await?;
        let path = self.cache_path(id);
        let staging = path.with_extension("json.partial");
        tokio::fs::write(&staging, bytes).await?;
        tokio::fs::rename(&staging, &path)
            .await
            .with_context(|| format!("cache plugin index {}", path.display()))
    }

    fn cache_path(&self, id: &str) -> PathBuf {
        // Ids are `official` or a UUID this service minted, so the name cannot leave the root.
        self.0.root.join(format!("index-{id}.json"))
    }

    pub(crate) fn downloads(&self) -> PathBuf {
        self.0.root.join("downloads")
    }
}

/// The index address of a repository row.
fn repository_url(repository: &PluginRepository) -> Result<url::Url, RepositoryError> {
    match &repository.url {
        None if repository.is_official() => parse_https(OFFICIAL_INDEX_URL),
        Some(url) => parse_https(url),
        None => Err(RepositoryError::InvalidUrl),
    }
}

/// An absolute https URL with a host and no credentials, at most 1,024 bytes.
pub fn parse_https(value: &str) -> Result<url::Url, RepositoryError> {
    let value = value.trim();
    if value.len() > 1024 {
        return Err(RepositoryError::InvalidUrl);
    }
    let url = url::Url::parse(value).map_err(|_| RepositoryError::InvalidUrl)?;
    if url.scheme() != "https"
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(RepositoryError::InvalidUrl);
    }
    Ok(url)
}

/// The key ids an index's envelope names, read without trusting anything else in it.
fn signature_key_ids(bytes: &[u8]) -> Vec<String> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        signatures: Vec<Signature>,
    }
    #[derive(Deserialize)]
    struct Signature {
        key_id: String,
    }
    serde_json::from_slice::<Envelope>(bytes)
        .map(|envelope| {
            let mut ids: Vec<String> = envelope
                .signatures
                .into_iter()
                .map(|signature| signature.key_id)
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .collect();
            ids.dedup();
            ids.truncate(8);
            ids
        })
        .unwrap_or_default()
}

fn base64_key(key: &rd_sign::VerifyingKey) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(key.as_bytes())
}

/// Whether `id` is the built-in repository.
#[must_use]
pub fn is_official(id: &str) -> bool {
    id == OFFICIAL_REPOSITORY_ID
}

/// Offers, updates and downloads for the entries of one index.
pub(crate) fn find_entry<'a>(
    index: &'a PluginIndex,
    plugin_id: &str,
    version: &str,
) -> Option<&'a IndexPackage> {
    index
        .packages
        .iter()
        .find(|entry| entry.id.to_string() == plugin_id && entry.version == version)
}

#[cfg(test)]
#[path = "repository_tests.rs"]
mod tests;

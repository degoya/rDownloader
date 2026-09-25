//! The managed tool store as one service: what is installed, what is active, and the four
//! operations that change either.
//!
//! Ordering is the interesting part. Activation flips a pointer and takes effect for the next
//! job immediately; a job already running keeps the version it resolved, because it holds a
//! lease and nothing removes a leased version. That is what "running jobs keep their tool
//! version" means here, and it needs no per-job column anywhere — the lease *is* the record,
//! and it disappears exactly when the process that needed it does.
//!
//! Nothing in this module touches a system-installed binary. The store owns
//! `<data>/tools/**` and only that; an explicitly configured path still wins over everything
//! managed, and a tool with no managed version resolves through the vendor folders and `PATH`
//! precisely as it did before this crate existed.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use chrono::Utc;
use rd_core::ManagedToolSettings;
use rd_db::Database;

use crate::{
    activation, compat,
    download::{self, executable_name},
    error::ToolError,
    lease::LeaseRegistry,
    manifest::{self, ToolEntry, ToolManifest, is_managed_tool},
    platform,
    store::{KEPT_VERSIONS_PER_TOOL, ToolStore},
};

/// File name of the cached copy of the last manifest that verified.
const CACHED_MANIFEST: &str = "manifest.json";

/// How long a manifest refresh may take.
const REFRESH_TIMEOUT_SECONDS: u64 = 30;

/// Largest manifest this will read, so a hostile endpoint cannot answer with a stream.
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// The active managed version of one tool.
#[derive(Clone, Debug)]
struct ActiveTool {
    version: String,
    path: PathBuf,
}

/// What the status endpoint reports about one managed tool.
#[derive(Clone, Debug)]
pub struct ManagedToolStatus {
    /// One of [`crate::manifest::MANAGED_TOOLS`].
    pub name: String,
    /// The version the managed stage of the lookup currently answers with.
    pub active_version: Option<String>,
    /// The executable that version points at.
    pub active_path: Option<String>,
    /// Every version on disk, newest install first.
    pub installed_versions: Vec<String>,
    /// The newest version the manifest offers for this platform and application version.
    pub available_version: Option<String>,
    /// Whether a rollback has somewhere to go.
    pub can_roll_back: bool,
}

struct Inner {
    database: Database,
    store: ToolStore,
    settings: RwLock<ManagedToolSettings>,
    manifest: RwLock<ToolManifest>,
    active: RwLock<HashMap<String, ActiveTool>>,
    leases: LeaseRegistry,
    client: reqwest::Client,
    app_version: String,
}

/// Installs, verifies, activates and rolls back managed external tools.
#[derive(Clone)]
pub struct ManagedToolService(Arc<Inner>);

impl ManagedToolService {
    /// Builds a service over `root`, which is `<data>/tools`.
    ///
    /// The compiled-in manifest is the starting point. A build whose embedded manifest does
    /// not verify is a broken build, and the safe reading of that is "this installation
    /// manages nothing" — not "install from an unverified document".
    #[must_use]
    pub fn new(database: Database, root: PathBuf, settings: ManagedToolSettings) -> Self {
        let manifest = match manifest::embedded(Utc::now()) {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::error!(%error, "the compiled-in tool manifest does not verify");
                empty_manifest()
            }
        };
        compat::adopt_manifest(&manifest);
        compat::set_overrides(&settings.tool_compatibility_overrides);
        Self(Arc::new(Inner {
            database,
            store: ToolStore::new(root),
            settings: RwLock::new(settings),
            manifest: RwLock::new(manifest),
            active: RwLock::new(HashMap::new()),
            leases: LeaseRegistry::new(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(
                    download::DOWNLOAD_TIMEOUT_SECONDS,
                ))
                .build()
                .unwrap_or_default(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
        }))
    }

    /// Reads the pointers from disk, clears staging leftovers and loads the cached manifest.
    ///
    /// Called once at startup, before the resolver is registered, so the first job to look a
    /// tool up already sees the managed stage rather than racing it.
    pub async fn load(&self) {
        let cached = self.0.store.root().join(CACHED_MANIFEST);
        if let Ok(bytes) = tokio::fs::read(&cached).await {
            // Verified with no known sequence on purpose. This document was accepted once
            // already and its sequence *is* the stored floor, so passing that floor back in
            // would make the cache a replay of itself and every restart would fall back to
            // the built-in manifest. Signature, schema and expiry are all still checked.
            match manifest::verify(&bytes, None, Utc::now()) {
                Ok(cached_manifest) if cached_manifest.sequence >= self.manifest().sequence => {
                    self.replace_manifest(cached_manifest);
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "cached tool manifest does not verify; falling back to the built-in one"
                    );
                    let _ = tokio::fs::remove_file(&cached).await;
                }
            }
        }
        for name in manifest::MANAGED_TOOLS {
            self.0.store.sweep_staging(name).await;
            self.reload_active(name).await;
        }
    }

    /// Replaces the settings this service acts on.
    pub fn apply_settings(&self, settings: ManagedToolSettings) {
        // The override list is a process-wide fact, the same way the manifest in force is:
        // a runner asking whether a capability is blocked must get the answer the settings
        // currently give, without every runner having to hold this service.
        compat::set_overrides(&settings.tool_compatibility_overrides);
        if let Ok(mut current) = self.0.settings.write() {
            *current = settings;
        }
    }

    /// A snapshot of the manifest currently in force.
    #[must_use]
    pub fn manifest(&self) -> ToolManifest {
        self.0
            .manifest
            .read()
            .map(|manifest| manifest.clone())
            .unwrap_or_else(|_| empty_manifest())
    }

    /// Replaces the manifest in force **without verifying it**.
    ///
    /// Everything on the production path goes through [`Self::refresh_manifest`], which
    /// verifies the signature and the freshness rule before anything reaches here. This exists
    /// for the tests that have to drive the install path against a local server, and for
    /// tooling that already verified the document itself. Hidden so it does not read as part
    /// of the supported surface.
    #[doc(hidden)]
    pub fn adopt_unverified_manifest(&self, manifest: ToolManifest) {
        self.replace_manifest(manifest);
    }

    /// Whether managed tools are switched on.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.0
            .settings
            .read()
            .is_ok_and(|settings| settings.managed_tools_enabled)
    }

    /// Status of every managed tool.
    pub async fn status(&self) -> Vec<ManagedToolStatus> {
        let manifest = self.manifest();
        let history = self
            .0
            .database
            .list_managed_tools()
            .await
            .unwrap_or_default();
        let mut statuses = Vec::with_capacity(manifest::MANAGED_TOOLS.len());
        for name in manifest::MANAGED_TOOLS {
            let active = self.active_tool(name);
            let installed = self.ordered_versions(name, &history).await;
            let available = manifest
                .newest_release(name, platform::current(), &self.0.app_version)
                .map(|entry| entry.version.clone());
            let can_roll_back = installed.iter().any(|version| {
                Some(version.as_str()) != active.as_ref().map(|tool| tool.version.as_str())
            });
            statuses.push(ManagedToolStatus {
                name: (*name).to_owned(),
                active_version: active.as_ref().map(|tool| tool.version.clone()),
                active_path: active
                    .as_ref()
                    .map(|tool| tool.path.to_string_lossy().into_owned()),
                installed_versions: installed,
                available_version: available,
                can_roll_back,
            });
        }
        statuses
    }

    /// Fetches, verifies and adopts a newer manifest from the configured URL.
    ///
    /// Refuses a document that is not signed by the compiled-in tool-manifest root, and one
    /// whose sequence is not above what this installation has already accepted. The accepted
    /// sequence is persisted before the manifest is adopted, so a crash between the two
    /// leaves the replay floor raised rather than lowered.
    pub async fn refresh_manifest(&self) -> Result<ToolManifest, ToolError> {
        self.require_enabled()?;
        let url = self
            .0
            .settings
            .read()
            .ok()
            .and_then(|settings| settings.managed_tools_manifest_url.clone())
            .ok_or_else(|| {
                ToolError::Other(anyhow::anyhow!("no tool manifest URL is configured"))
            })?;
        if !url.starts_with("https://") {
            return Err(ToolError::ManifestUntrusted(
                "the tool manifest URL is not https".to_owned(),
            ));
        }
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(REFRESH_TIMEOUT_SECONDS),
            fetch_manifest(&self.0.client, &url),
        )
        .await
        .map_err(|_| ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: format!("no answer within {REFRESH_TIMEOUT_SECONDS} seconds"),
        })??;
        let known = self.known_sequence().await;
        let manifest = manifest::verify(&bytes, known, Utc::now())?;
        self.0
            .database
            .accept_tool_manifest(
                i64::try_from(manifest.sequence).unwrap_or(i64::MAX),
                manifest.issued_at.to_rfc3339(),
            )
            .await?;
        let cached = self.0.store.root().join(CACHED_MANIFEST);
        if let Some(parent) = cached.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&cached, &bytes).await;
        self.replace_manifest(manifest.clone());
        Ok(manifest)
    }

    /// Downloads and installs one version, without touching what is active.
    ///
    /// `version` of `None` takes the newest build the manifest offers for this platform and
    /// this application version. Nothing is visible to the resolver until the bytes have
    /// hashed to what the manifest said they would.
    ///
    /// The first version of a tool activates itself: an installation that downloaded and
    /// verified a binary and then pointed at nothing would be a puzzle, not a safety measure.
    pub async fn install(&self, name: &str, version: Option<&str>) -> Result<String, ToolError> {
        self.require_enabled()?;
        if !is_managed_tool(name) {
            return Err(ToolError::NotManaged(name.to_owned()));
        }
        let entry = self.release(name, version)?;
        if self.0.store.is_installed(name, &entry.version).await {
            return Ok(entry.version);
        }
        let staging = self.0.store.stage(name).await?;
        if let Err(error) = download::fetch_into(&self.0.client, &entry, staging.path()).await {
            staging.discard().await;
            return Err(error);
        }
        self.0.store.promote(staging, name, &entry.version).await?;
        self.0
            .database
            .record_managed_tool(rd_db::NewManagedTool {
                name: name.to_owned(),
                version: entry.version.clone(),
                source_url: entry.url.clone(),
                sha256: entry.sha256.clone(),
            })
            .await?;
        if self.active_tool(name).is_none() {
            self.activate(name, &entry.version).await?;
        }
        Ok(entry.version)
    }

    /// Points the managed stage of the lookup at `version`.
    ///
    /// Takes effect for the next job that resolves the tool. A job already running keeps the
    /// binary it resolved, because its lease keeps that version on disk.
    pub async fn activate(&self, name: &str, version: &str) -> Result<(), ToolError> {
        if !is_managed_tool(name) {
            return Err(ToolError::NotManaged(name.to_owned()));
        }
        if !self.0.store.is_installed(name, version).await {
            return Err(ToolError::VersionNotInstalled {
                name: name.to_owned(),
                version: version.to_owned(),
            });
        }
        let directory = self.0.store.tool_directory(name)?;
        activation::write(&directory, version).await?;
        self.reload_active(name).await;
        self.prune(name).await;
        Ok(())
    }

    /// Returns to the most recently installed version other than the active one.
    pub async fn rollback(&self, name: &str) -> Result<String, ToolError> {
        if !is_managed_tool(name) {
            return Err(ToolError::NotManaged(name.to_owned()));
        }
        let active = self.active_tool(name).map(|tool| tool.version);
        let history = self
            .0
            .database
            .list_managed_tools()
            .await
            .unwrap_or_default();
        let previous = self
            .ordered_versions(name, &history)
            .await
            .into_iter()
            .find(|version| Some(version) != active.as_ref())
            .ok_or_else(|| ToolError::NothingToRollBackTo {
                name: name.to_owned(),
            })?;
        self.activate(name, &previous).await?;
        Ok(previous)
    }

    /// Removes one installed version.
    ///
    /// Refuses the active version and any version a running job still holds a lease on: that
    /// refusal is the whole reason leases exist.
    pub async fn remove_version(&self, name: &str, version: &str) -> Result<(), ToolError> {
        if !is_managed_tool(name) {
            return Err(ToolError::NotManaged(name.to_owned()));
        }
        if self
            .active_tool(name)
            .is_some_and(|tool| tool.version == version)
            || self.0.leases.is_leased(name, version)
        {
            return Err(ToolError::InUse {
                name: name.to_owned(),
                version: version.to_owned(),
            });
        }
        if !self.0.store.is_installed(name, version).await {
            return Err(ToolError::VersionNotInstalled {
                name: name.to_owned(),
                version: version.to_owned(),
            });
        }
        self.0.store.remove_version(name, version).await?;
        let _ = self
            .0
            .database
            .forget_managed_tool(name.to_owned(), version.to_owned())
            .await;
        Ok(())
    }

    /// Whether a version is currently held by a running job. Exposed for diagnostics and
    /// for the tests that prove a leased version survives a prune.
    #[must_use]
    pub fn is_leased(&self, name: &str, version: &str) -> bool {
        self.0.leases.is_leased(name, version)
    }

    /// The tool store's root directory.
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        self.0.store.root()
    }

    fn require_enabled(&self) -> Result<(), ToolError> {
        if self.is_enabled() {
            Ok(())
        } else {
            Err(ToolError::Disabled)
        }
    }

    /// The manifest entry to install, by exact version or newest.
    fn release(&self, name: &str, version: Option<&str>) -> Result<ToolEntry, ToolError> {
        let manifest = self.manifest();
        let platform = platform::current();
        let entry = match version {
            Some(version) => manifest.release(name, version, platform, &self.0.app_version),
            None => manifest.newest_release(name, platform, &self.0.app_version),
        };
        entry.cloned().ok_or_else(|| ToolError::NoRelease {
            name: name.to_owned(),
        })
    }

    async fn known_sequence(&self) -> Option<u64> {
        self.0
            .database
            .tool_manifest_state()
            .await
            .ok()
            .flatten()
            .and_then(|state| u64::try_from(state.sequence).ok())
    }

    fn replace_manifest(&self, manifest: ToolManifest) {
        // Rules travel with the builds they talk about, so adopting a manifest is also
        // adopting its policy — and failing to read that policy degrades to the compiled-in
        // base rather than to no policy at all.
        compat::adopt_manifest(&manifest);
        if let Ok(mut current) = self.0.manifest.write() {
            *current = manifest;
        }
    }

    fn active_tool(&self, name: &str) -> Option<ActiveTool> {
        self.0.active.read().ok()?.get(name).cloned()
    }

    /// Re-reads the pointer for one tool and updates the in-memory answer the resolver gives.
    async fn reload_active(&self, name: &str) {
        let Ok(directory) = self.0.store.tool_directory(name) else {
            return;
        };
        let resolved = match activation::read(&directory).await {
            Some(pointer) => {
                let path = directory.join(&pointer.version).join(executable_name(name));
                match tokio::fs::metadata(&path).await {
                    Ok(_) => Some(ActiveTool {
                        version: pointer.version,
                        path,
                    }),
                    // A pointer at a version whose binary is gone is worse than no pointer:
                    // the lookup would report a managed tool and then fail to run it.
                    Err(_) => {
                        tracing::warn!(
                            tool = name,
                            "the active managed version is missing on disk"
                        );
                        None
                    }
                }
            }
            None => None,
        };
        if let Ok(mut active) = self.0.active.write() {
            match resolved {
                Some(tool) => active.insert(name.to_owned(), tool),
                None => active.remove(name),
            };
        }
    }

    /// Installed versions of `name`, newest install first, with anything the database does
    /// not know about appended so a hand-copied directory is still visible.
    async fn ordered_versions(
        &self,
        name: &str,
        history: &[rd_db::ManagedToolRecord],
    ) -> Vec<String> {
        let on_disk = self
            .0
            .store
            .installed_versions(name)
            .await
            .unwrap_or_default();
        let mut ordered: Vec<String> = history
            .iter()
            .filter(|record| record.name == name)
            .map(|record| record.version.clone())
            .filter(|version| on_disk.contains(version))
            .collect();
        for version in on_disk {
            if !ordered.contains(&version) {
                ordered.push(version);
            }
        }
        ordered
    }

    /// Drops versions beyond [`KEPT_VERSIONS_PER_TOOL`], never the active one and never one a
    /// running job holds.
    async fn prune(&self, name: &str) {
        let history = self
            .0
            .database
            .list_managed_tools()
            .await
            .unwrap_or_default();
        let active = self.active_tool(name).map(|tool| tool.version);
        let mut kept = 0;
        for version in self.ordered_versions(name, &history).await {
            if Some(&version) == active.as_ref() {
                kept += 1;
                continue;
            }
            kept += 1;
            if kept <= KEPT_VERSIONS_PER_TOOL {
                continue;
            }
            if let Err(error) = self.remove_version(name, &version).await {
                tracing::debug!(tool = name, %version, %error, "kept an old managed tool version");
            }
        }
    }
}

impl rd_core::ManagedToolResolver for ManagedToolService {
    fn resolve(&self, name: &str) -> Option<rd_core::ManagedTool> {
        // The lease is taken while the `active` read guard is *still held*, and that is what
        // closes the window. Reading the pointer first and leasing afterwards left a gap:
        // `activate` → `reload_active` → `prune` → `remove_version` could run in it, see the
        // version neither active nor leased, and delete the directory the caller had just
        // resolved — the runner then spawned a path that no longer existed, which is exactly
        // the failure leases exist to prevent. Holding the guard keeps `reload_active` out,
        // because it needs the write guard, so the count is already up before any activation
        // that could feed this version to `prune` can land.
        //
        // Lock order is `active` then `leases`, the same direction `remove_version` walks
        // them, and nothing anywhere takes them the other way round — `reload_active` touches
        // only `active`, a dropped lease only `leases`. So this cannot deadlock. Both are
        // `std` locks in a synchronous function; no await is held across either.
        let active = self.0.active.read().ok()?;
        let tool = active.get(name)?;
        let lease = self.0.leases.acquire(name, &tool.version);
        Some(rd_core::ManagedTool {
            path: tool.path.clone(),
            version: tool.version.clone(),
            lease,
        })
    }
}

/// A manifest that offers nothing — the safe state when no verified one is available.
fn empty_manifest() -> ToolManifest {
    ToolManifest {
        schema_version: manifest::TOOL_MANIFEST_SCHEMA_VERSION,
        sequence: 0,
        issued_at: chrono::DateTime::UNIX_EPOCH,
        not_after: None,
        tools: Vec::new(),
        compatibility: Vec::new(),
    }
}

/// Reads a manifest document, bounded so a hostile endpoint cannot answer with a stream.
async fn fetch_manifest(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, ToolError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: error.to_string(),
        })?;
    if !response.status().is_success() {
        return Err(ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: format!("the server answered {}", response.status()),
        });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: error.to_string(),
        })?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: format!("the manifest exceeds {MAX_MANIFEST_BYTES} bytes"),
        });
    }
    Ok(bytes.to_vec())
}

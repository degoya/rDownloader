//! Plugins for the version an update installs (RD-1260-02).
//!
//! A release that raises the `rdownloader:plugin` contract refuses every installed package built
//! for the one before (RD-1251-01). So the preparation before an update hands over
//! ([`PluginRepositoryService::prepare_update`], called by `pre_update_service`) fetches, for each
//! installed plugin the new version would refuse, the newest package the repositories offer for
//! the new contract, and leaves it in `<root>/prepared/<version>/`. The new version's start
//! installs those before it reads the bundle ([`install_prepared`]), so the switch is one step and
//! the bundle is only the fallback for what no repository had.
//!
//! The folder is written as `<version>.partial` and renamed once complete, so a stop in between
//! leaves nothing a start reads. Every start removes every preparation after taking its own: a
//! version that never started — the update failed or was taken back — leaves nothing behind for
//! the running version or the next attempt to trip over.

use std::path::Path;

use rd_core::PluginId;
use rd_db::{Database, PluginRepositoryInstall};
use serde::{Deserialize, Serialize};

use super::{
    Offer, PackageCompatibility, PluginRepositoryService, RepositoryError, compatibility_with,
    offers::{Newest, newest_installed},
};
use crate::{APP_VERSION, PluginInstaller};

/// The folder under the repositories' root the preparations are kept in.
const PREPARED: &str = "prepared";
/// Which repository each prepared package came from, by package digest.
const SOURCES: &str = "sources.json";

/// The version an update installs, and the contracts its plugin host speaks.
#[derive(Clone, Copy, Debug)]
pub struct UpdateTarget<'a> {
    /// A plain version, letters, digits, `.`, `_`, `+` and `-`; it names the folder.
    pub version: &'a str,
    /// The `rdownloader:plugin` contract versions, as `SUPPORTED_API_VERSIONS` lists them.
    pub contracts: &'a [String],
}

/// One package fetched for the new version.
#[derive(Clone, Debug, Serialize)]
pub struct PreparedPlugin {
    pub plugin_id: PluginId,
    pub name: String,
    pub version: String,
    pub repository_id: String,
}

/// What a preparation fetched, and what it could not.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PreparedPlugins {
    pub prepared: Vec<PreparedPlugin>,
    /// Names of installed plugins the new version refuses that no repository offered a package
    /// for, or whose package could not be fetched: the bundle replaces them at the next start
    /// where it can, and the plugin manager offers what the repositories have later.
    pub missing: Vec<String>,
}

/// Where a prepared package came from; written beside the packages.
#[derive(Debug, Deserialize, Serialize)]
struct PreparedSource {
    digest: String,
    plugin_id: String,
    version: String,
    repository_id: String,
}

/// What a start took from the preparation its predecessor left.
#[derive(Debug, Default)]
pub struct PreparedInstallReport {
    pub installed: Vec<(PluginId, String)>,
    /// `(file name, reason)` of a prepared package that was not installed.
    pub rejected: Vec<(String, String)>,
}

/// Whether `version` can name a folder: what `pre_update::is_version` accepts.
fn plain_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && !version.starts_with('.')
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

/// The newest package for the new version that may stand in for `base`: the same plugin, signed
/// by the same key, runnable there, not withdrawn, and asking for nothing `base` was not granted —
/// it installs at the next start without a preview, as the bundle's packages do.
fn candidate<'a>(
    offers: &'a [Offer],
    id: &str,
    base: &Newest,
    contracts: &[&str],
    version: &str,
) -> Option<&'a Offer> {
    let mut best: Option<(&Offer, semver::Version)> = None;
    for offer in offers {
        let fits = offer.entry.id.to_string() == id
            && offer.compatibility != PackageCompatibility::Withdrawn
            && compatibility_with(&offer.entry, contracts, version)
                == PackageCompatibility::Compatible
            && base.fingerprint.as_deref() == Some(offer.entry.publisher.fingerprint.as_str())
            && !offer.entry.permissions.widens(&base.permissions);
        let Some(found) = fits
            .then(|| semver::Version::parse(&offer.entry.version).ok())
            .flatten()
        else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, known)| found > *known) {
            best = Some((offer, found));
        }
    }
    best.map(|(offer, _)| offer)
}

impl PluginRepositoryService {
    /// Fetches the packages `target` needs for the installed plugins it would refuse, and
    /// leaves them where its start installs them from.
    ///
    /// Nothing is fetched — not even an index — when every installed plugin already speaks one
    /// of the target's contracts, which is every update without a contract change. A package that
    /// cannot be fetched is named in `missing` and costs nothing else: the update goes ahead and
    /// the bundle stays the fallback.
    ///
    /// # Errors
    ///
    /// A target version that names no folder, a list of the installed plugins that cannot be
    /// read, or a folder that cannot be written.
    pub async fn prepare_update(
        &self,
        target: UpdateTarget<'_>,
    ) -> Result<PreparedPlugins, RepositoryError> {
        let root = self.0.root.join(PREPARED);
        // An earlier preparation is replaced whole, whichever version it was for.
        if let Err(error) = tokio::fs::remove_dir_all(&root).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(%error, folder = %root.display(), "an earlier update preparation could not be removed");
        }
        if !plain_version(target.version) {
            return Err(RepositoryError::Other(anyhow::anyhow!(
                "{:?} names no folder",
                target.version
            )));
        }
        let contracts: Vec<&str> = target.contracts.iter().map(String::as_str).collect();
        let bases = newest_installed(&self.installer().list_update_bases().await?);
        let mut needed: Vec<(&String, &Newest)> = bases
            .iter()
            .filter(|(_, base)| !contracts.contains(&base.api_version.as_str()))
            .collect();
        let mut outcome = PreparedPlugins::default();
        if needed.is_empty() {
            return Ok(outcome);
        }
        needed.sort_by(|left, right| left.1.name.cmp(&right.1.name));
        // The new release's index is what lists the new contract's packages.
        self.refresh_all().await;
        let offers = self.offers().await?;
        let mut chosen = Vec::new();
        for (id, base) in needed {
            match candidate(&offers, id, base, &contracts, target.version) {
                Some(offer) => chosen.push(offer.clone()),
                None => outcome.missing.push(base.name.clone()),
            }
        }
        if chosen.is_empty() {
            return Ok(outcome);
        }
        let staging = root.join(format!("{}.partial", target.version));
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(anyhow::Error::from)?;
        let mut sources = Vec::new();
        for offer in chosen {
            let id = offer.entry.id.to_string();
            let (offer, bytes) = match self
                .download(&offer.repository_id, &id, &offer.entry.version)
                .await
            {
                Ok(found) => found,
                Err(error) => {
                    tracing::warn!(plugin = %offer.entry.name, version = %offer.entry.version, code = error.code(), %error, "a plugin for the update could not be fetched; the bundle stays its fallback");
                    outcome.missing.push(offer.entry.name.clone());
                    continue;
                }
            };
            let digest = offer.entry.package_digest.clone();
            tokio::fs::write(staging.join(format!("{digest}.rdplug")), &bytes)
                .await
                .map_err(anyhow::Error::from)?;
            sources.push(PreparedSource {
                digest,
                plugin_id: id,
                version: offer.entry.version.clone(),
                repository_id: offer.repository_id.clone(),
            });
            outcome.prepared.push(PreparedPlugin {
                plugin_id: offer.entry.id,
                name: offer.entry.name.clone(),
                version: offer.entry.version.clone(),
                repository_id: offer.repository_id.clone(),
            });
        }
        if sources.is_empty() {
            if let Err(error) = tokio::fs::remove_dir_all(&root).await {
                tracing::debug!(%error, folder = %root.display(), "an empty update preparation could not be removed");
            }
            return Ok(outcome);
        }
        let listed = serde_json::to_vec_pretty(&sources).map_err(anyhow::Error::from)?;
        tokio::fs::write(staging.join(SOURCES), listed)
            .await
            .map_err(anyhow::Error::from)?;
        // A stop here leaves only the staging name, which no start reads and every start
        // removes; the bundle then replaces what the new contract refuses (recovery matrix).
        rd_core::failpoint!("plugin.before_prepared_published", || {
            RepositoryError::Other(anyhow::anyhow!("crash point"))
        });
        tokio::fs::rename(&staging, root.join(target.version))
            .await
            .map_err(anyhow::Error::from)?;
        Ok(outcome)
    }
}

/// Installs what the version before this one prepared for it, then removes every preparation.
///
/// Run by the start before the bundle is read, with `root` the repositories' folder
/// ([`DIRECTORY`](super::DIRECTORY) in the data directory). A prepared package installs when it is
/// newer than every installed version of its plugin, runnable or refused; a plugin removed in the
/// meantime stays removed. Each package is verified like any install, and one that does not verify
/// is reported and skipped — the bundle then replaces its plugin where it can. Never fails: a
/// start does not wait on a preparation.
pub async fn install_prepared(
    installer: &PluginInstaller,
    database: &Database,
    root: &Path,
) -> PreparedInstallReport {
    let prepared = root.join(PREPARED);
    let mut report = PreparedInstallReport::default();
    let own = prepared.join(APP_VERSION);
    if tokio::fs::metadata(&own)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        install_from(installer, database, &own, &mut report).await;
    }
    match tokio::fs::remove_dir_all(&prepared).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(%error, folder = %prepared.display(), "the plugins prepared for an update could not be removed");
        }
        _ => {}
    }
    report
}

async fn install_from(
    installer: &PluginInstaller,
    database: &Database,
    folder: &Path,
    report: &mut PreparedInstallReport,
) {
    let sources: Vec<PreparedSource> = tokio::fs::read(folder.join(SOURCES))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let installed = match installer.installed_or_refused_versions().await {
        Ok(installed) => installed,
        Err(error) => {
            tracing::warn!(%error, "the installed plugins could not be read; the prepared ones wait for nothing");
            return;
        }
    };
    let listed = folder.to_path_buf();
    let files = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<std::path::PathBuf>> {
        let mut files: Vec<_> = std::fs::read_dir(&listed)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "rdplug")
            })
            .collect();
        files.sort();
        Ok(files)
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default();
    for path in files {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("?")
            .to_owned();
        let verifier = installer.verifier.clone();
        let candidate = path.clone();
        let package =
            match tokio::task::spawn_blocking(move || verifier.verify_file(&candidate)).await {
                Ok(Ok(package)) => package,
                Ok(Err(error)) => {
                    report.rejected.push((name, error.to_string()));
                    continue;
                }
                Err(error) => {
                    report.rejected.push((name, error.to_string()));
                    continue;
                }
            };
        let id = package.manifest.id;
        let Ok(prepared) = semver::Version::parse(&package.manifest.version) else {
            continue;
        };
        let versions: Vec<semver::Version> = installed
            .iter()
            .filter(|(installed, _)| *installed == id)
            .filter_map(|(_, version)| semver::Version::parse(version).ok())
            .collect();
        if versions.is_empty() || versions.iter().any(|version| *version >= prepared) {
            continue;
        }
        let digest = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        match installer.install_verified(package).await {
            Ok(done) => {
                let version = done.manifest.version.clone();
                if let Some(source) = sources.iter().find(|source| source.digest == digest) {
                    record_delivery(database, source).await;
                }
                report.installed.push((id, version));
            }
            Err(error) => report.rejected.push((name, format!("{error:#}"))),
        }
    }
}

/// Records which repository delivered a prepared package, so a later withdrawal of that
/// repository reaches it as it reaches every other install from there.
async fn record_delivery(database: &Database, source: &PreparedSource) {
    let recorded = database
        .record_plugin_repository_install(PluginRepositoryInstall {
            plugin_id: source.plugin_id.clone(),
            version: source.version.clone(),
            digest: source.digest.clone(),
            repository_id: source.repository_id.clone(),
            installed_at: chrono::Utc::now().to_rfc3339(),
        })
        .await;
    if let Err(error) = recorded {
        tracing::warn!(%error, plugin = %source.plugin_id, "could not record which repository a prepared plugin came from");
    }
}

//! What the enabled repositories offer, which of it is an update, and fetching one package
//! so that it provably is the one the index described (RD-140-01).

use std::collections::HashMap;

use rd_db::PluginRepositoryInstall;
use serde::Serialize;

use super::{PluginRepositoryService, RepositoryError, UpdatePolicy, find_entry};
use crate::{
    APP_VERSION, PluginManifest, PluginVerifier, SUPPORTED_API_VERSIONS, format_package_digest,
    index::{IndexPackage, Permissions},
    key_fingerprint,
    preview::{archive_digest, preview_package},
};

/// Whether this build can run an offered package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageCompatibility {
    Compatible,
    /// Built against a `rdownloader:plugin` contract this build does not speak.
    ContractUnsupported,
    /// Needs a newer rDownloader than this one.
    AppTooOld,
    /// Withdrawn by its digest or by its signing key.
    Withdrawn,
}

/// One package an enabled repository offers.
#[derive(Clone, Debug, Serialize)]
pub struct Offer {
    pub repository_id: String,
    pub repository_name: String,
    pub official: bool,
    pub entry: IndexPackage,
    pub compatibility: PackageCompatibility,
    /// The newest version of this plugin installed here, if any.
    pub installed_version: Option<String>,
}

/// An offer that is newer than every installed version of the same plugin, from the same
/// publisher.
#[derive(Clone, Debug, Serialize)]
pub struct Update {
    pub offer: Offer,
    pub installed_version: String,
    pub policy: UpdatePolicy,
    /// Whether the update asks for a capability, domain or stream host the installed version
    /// does not have. Such an update is never installed automatically: a new permission is
    /// shown before it is granted, so it waits for a click whatever the policy says.
    pub adds_permissions: bool,
    /// Which capabilities, domains and stream hosts those are (RD-160-09); empty exactly when
    /// `adds_permissions` is false.
    pub added_permissions: Permissions,
}

impl Update {
    /// Whether the refresh installs this on its own: policy *automatic*, and no permission the
    /// installed version lacks. Everything else waits for a click.
    #[must_use]
    pub fn installs_itself(&self) -> bool {
        self.policy == UpdatePolicy::Automatic && !self.adds_permissions
    }
}

/// Whether this build can run `entry`, judged from the index alone.
#[must_use]
pub fn compatibility(entry: &IndexPackage) -> PackageCompatibility {
    if !SUPPORTED_API_VERSIONS.contains(&entry.api_version.as_str()) {
        return PackageCompatibility::ContractUnsupported;
    }
    let too_old = entry.min_app_version.as_deref().is_some_and(|minimum| {
        match (
            semver::Version::parse(minimum),
            semver::Version::parse(APP_VERSION),
        ) {
            (Ok(minimum), Ok(current)) => current < minimum,
            _ => true,
        }
    });
    if too_old {
        PackageCompatibility::AppTooOld
    } else {
        PackageCompatibility::Compatible
    }
}

/// The newest installed version of one plugin, and the key it is signed with.
struct Newest {
    version: semver::Version,
    text: String,
    fingerprint: Option<String>,
    permissions: Permissions,
}

/// The newest installed version of each plugin id. "Newest" rather than "active": an update is
/// a version newer than every installed one, the rule the bundled packages follow, so a version
/// kept for a rollback is never offered again.
fn newest_installed(installed: &[PluginManifest]) -> HashMap<String, Newest> {
    let mut newest: HashMap<String, Newest> = HashMap::new();
    for manifest in installed {
        let Ok(version) = semver::Version::parse(&manifest.version) else {
            continue;
        };
        let id = manifest.id.to_string();
        if newest.get(&id).is_none_or(|known| version > known.version) {
            let fingerprint = manifest
                .verifying_key()
                .ok()
                .map(|key| key_fingerprint(&key));
            newest.insert(
                id,
                Newest {
                    version,
                    text: manifest.version.clone(),
                    fingerprint,
                    permissions: Permissions::of(manifest),
                },
            );
        }
    }
    newest
}

impl PluginRepositoryService {
    /// Every package the enabled repositories offer, official repository first.
    pub async fn offers(&self) -> anyhow::Result<Vec<Offer>> {
        let repositories = self.database().list_plugin_repositories().await?;
        let installed = newest_installed(&self.installer().list_installed().await?);
        let loaded = self.loaded();
        let verifier = self.installer().verifier();
        let mut offers = Vec::new();
        for repository in repositories.iter().filter(|repository| repository.enabled) {
            let Some(index) = loaded.get(&repository.id) else {
                continue;
            };
            for entry in &index.index.packages {
                let withdrawn =
                    entry.digest().ok().is_some_and(|digest| {
                        verifier.is_package_revoked(&digest).unwrap_or(false)
                    }) || verifier
                        .is_key_withdrawn(&entry.publisher.fingerprint)
                        .unwrap_or(false);
                offers.push(Offer {
                    repository_id: repository.id.clone(),
                    repository_name: repository.name.clone(),
                    official: repository.is_official(),
                    entry: entry.clone(),
                    compatibility: if withdrawn {
                        PackageCompatibility::Withdrawn
                    } else {
                        compatibility(entry)
                    },
                    installed_version: installed
                        .get(&entry.id.to_string())
                        .map(|newest| newest.text.clone()),
                });
            }
        }
        Ok(offers)
    }

    /// The newest compatible offer per installed plugin that is newer than every installed
    /// version and signed by the key the installed version is signed with.
    ///
    /// The same key, because an update replaces code the person already trusts: a newer
    /// version under another publisher's key is a different plugin that happens to share an
    /// id, and it is offered like any other package — with its own preview — never as an update.
    pub async fn updates(&self) -> anyhow::Result<Vec<Update>> {
        let installed = newest_installed(&self.installer().list_installed().await?);
        let mut best: HashMap<String, (semver::Version, Offer, Permissions)> = HashMap::new();
        for offer in self.offers().await? {
            if offer.compatibility != PackageCompatibility::Compatible {
                continue;
            }
            let id = offer.entry.id.to_string();
            let Some(current) = installed.get(&id) else {
                continue;
            };
            let Ok(version) = semver::Version::parse(&offer.entry.version) else {
                continue;
            };
            if version <= current.version
                || current.fingerprint.as_deref()
                    != Some(offer.entry.publisher.fingerprint.as_str())
            {
                continue;
            }
            // Repositories are listed official first, so on a tie the official offer stays.
            if best.get(&id).is_none_or(|(known, _, _)| version > *known) {
                let added = offer.entry.permissions.beyond(&current.permissions);
                best.insert(id, (version, offer, added));
            }
        }
        let mut updates = Vec::with_capacity(best.len());
        for (id, (_, offer, added_permissions)) in best {
            let installed_version = installed
                .get(&id)
                .map(|newest| newest.text.clone())
                .unwrap_or_default();
            updates.push(Update {
                policy: self.policy(&id).await,
                installed_version,
                offer,
                adds_permissions: !added_permissions.is_empty(),
                added_permissions,
            });
        }
        updates.sort_by(|left, right| left.offer.entry.name.cmp(&right.offer.entry.name));
        Ok(updates)
    }

    /// Downloads one offered package and proves it is the one the index describes: its size,
    /// then its content digest, before anything else reads it — and then that the entry tells
    /// the truth about it (see [`described_by`]).
    ///
    /// Kept under `downloads/` by digest, so the install that follows a preview does not fetch
    /// the same bytes twice; the file is checked again when it is read back, and the directory
    /// is cleared at every start.
    pub async fn download(
        &self,
        repository_id: &str,
        plugin_id: &str,
        version: &str,
    ) -> Result<(Offer, Vec<u8>), RepositoryError> {
        let repository = self
            .database()
            .plugin_repository(repository_id)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        if !repository.enabled {
            return Err(RepositoryError::Disabled);
        }
        let not_offered = || RepositoryError::NotOffered {
            id: plugin_id.to_owned(),
            version: version.to_owned(),
        };
        let loaded = self.loaded();
        let index = loaded.get(repository_id).ok_or_else(not_offered)?;
        let entry = find_entry(&index.index, plugin_id, version).ok_or_else(not_offered)?;
        let expected = entry.digest()?;
        let cached = self
            .downloads()
            .join(format!("{}.rdplug", format_package_digest(&expected)));
        let bytes = match tokio::fs::read(&cached).await {
            Ok(bytes) if matches_entry(&bytes, entry, &expected) => bytes,
            _ => {
                let url = entry.resolve_url(&index.url)?;
                let bytes = self.fetch(&url, entry.size).await?;
                if !matches_entry(&bytes, entry, &expected) {
                    return Err(RepositoryError::DigestMismatch);
                }
                if tokio::fs::create_dir_all(self.downloads()).await.is_ok() {
                    let _ = tokio::fs::write(&cached, &bytes).await;
                }
                bytes
            }
        };
        described_by(&bytes, entry, self.installer().verifier())
            .map_err(RepositoryError::NotAsDescribed)?;
        let offer = self
            .offers()
            .await?
            .into_iter()
            .find(|offer| {
                offer.repository_id == repository_id
                    && offer.entry.id.to_string() == plugin_id
                    && offer.entry.version == version
            })
            .ok_or_else(not_offered)?;
        Ok((offer, bytes))
    }

    /// Records that `offer` was installed, so a later withdrawal by its repository reaches it,
    /// and drops the download it was installed from.
    pub async fn record_install(&self, offer: &Offer) -> anyhow::Result<()> {
        let digest = offer.entry.package_digest.clone();
        let _ = tokio::fs::remove_file(self.downloads().join(format!("{digest}.rdplug"))).await;
        self.database()
            .record_plugin_repository_install(PluginRepositoryInstall {
                plugin_id: offer.entry.id.to_string(),
                version: offer.entry.version.clone(),
                digest,
                repository_id: offer.repository_id.clone(),
                installed_at: chrono::Utc::now().to_rfc3339(),
            })
            .await?;
        Ok(())
    }
}

fn matches_entry(bytes: &[u8], entry: &IndexPackage, expected: &[u8; 32]) -> bool {
    u64::try_from(bytes.len()).ok() == Some(entry.size)
        && archive_digest(bytes).is_ok_and(|digest| &digest == expected)
}

/// Holds an index entry to the package it names.
///
/// The digest proves the bytes are the ones the entry named; it proves nothing about what the
/// entry *says* about them, and the repository writes both. The offer list, the update check
/// and an automatic update read the entry, not the package: its publisher fingerprint decides
/// what counts as an update of an installed plugin, and its permissions are what the updates
/// list shows. So a third-party index that lists a package signed by its own key under the
/// fingerprint of the key an installed plugin is signed with would otherwise reach that plugin
/// as an "update" — installed automatically wherever the person trusts the third party's key
/// for anything. And an unsigned package is refused here whatever the verifier's mode: a
/// repository lists signed packages only, and development mode is for a developer's own files.
fn described_by(
    bytes: &[u8],
    entry: &IndexPackage,
    verifier: &PluginVerifier,
) -> Result<(), String> {
    let preview = preview_package(bytes, verifier).map_err(|error| format!("{error:#}"))?;
    let Some(publisher) = &preview.publisher else {
        return Err("the package is unsigned".to_owned());
    };
    let checks = [
        ("plugin id", preview.id == entry.id),
        ("name", preview.name == entry.name),
        ("version", preview.version == entry.version),
        (
            "plugin type",
            preview.plugin_type.as_str() == entry.plugin_type.as_str(),
        ),
        ("contract", preview.api_version == entry.api_version),
        (
            "signing key",
            publisher.fingerprint == entry.publisher.fingerprint
                && publisher.key_id == entry.publisher.key_id,
        ),
        ("author", publisher.author == entry.publisher.author),
        (
            "permissions",
            preview.permissions.same_as(&entry.permissions),
        ),
    ];
    match checks.into_iter().find(|(_, holds)| !holds) {
        Some((field, _)) => Err(format!("its {field} differs from what the index says")),
        None => Ok(()),
    }
}

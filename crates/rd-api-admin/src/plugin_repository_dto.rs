//! Request and response bodies of the plugin repository routes (RD-140-01).

use rd_plugin_host::{
    index::{IndexPackage, Permissions, Publisher},
    preview::{KeyStatus, PackagePreview},
    repository::{Offer, PackageCompatibility, Update, UpdatePolicy},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// One configured repository.
#[derive(Serialize, ToSchema)]
pub struct PluginRepositoryResponse {
    pub id: String,
    /// `official` or `third_party`.
    pub kind: String,
    pub name: String,
    /// The index address; the official one's is compiled in and shown here too.
    pub url: String,
    /// The approved repository key; `None` for the official repository, whose key is compiled in.
    pub key_id: Option<String>,
    pub fingerprint: Option<String>,
    pub enabled: bool,
    /// The highest index sequence accepted so far.
    pub sequence: Option<i64>,
    pub issued_at: Option<String>,
    /// When the index in use expires; `None` while none is loaded.
    pub expires_at: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_success_at: Option<String>,
    /// Stable code of the last refresh's failure.
    pub last_error: Option<String>,
}

impl PluginRepositoryResponse {
    pub(crate) fn from_row(
        row: rd_db::PluginRepository,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        Self {
            url: row
                .url
                .clone()
                .unwrap_or_else(|| rd_plugin_host::repository::OFFICIAL_INDEX_URL.to_owned()),
            id: row.id,
            kind: row.kind,
            name: row.name,
            key_id: row.key_id,
            fingerprint: row.fingerprint,
            enabled: row.enabled,
            sequence: row.sequence,
            issued_at: row.issued_at,
            expires_at: expires_at.map(|moment| moment.to_rfc3339()),
            last_checked_at: row.last_checked_at,
            last_success_at: row.last_success_at,
            last_error: row.last_error,
        }
    }
}

/// Every repository and the refresh interval.
#[derive(Serialize, ToSchema)]
pub struct PluginRepositoriesResponse {
    pub repositories: Vec<PluginRepositoryResponse>,
    /// Hours between two automatic refreshes.
    pub refresh_hours: u32,
}

/// Body of `POST /api/v1/plugins/repositories`.
#[derive(Deserialize, ToSchema)]
pub struct AddPluginRepositoryRequest {
    /// The index's `https://` address.
    pub url: String,
    /// The repository's Base64 Ed25519 public key, as its publisher publishes it.
    pub public_key: String,
    /// What to call it; the address's host when empty.
    #[serde(default)]
    pub name: Option<String>,
}

/// Confirmation of the fingerprint the client was shown for a repository key.
#[derive(Deserialize, IntoParams)]
pub struct AddPluginRepositoryQuery {
    /// Hex SHA-256 of the repository key, exactly as the 409 response reported it.
    #[serde(default)]
    pub trust_fingerprint: Option<String>,
}

/// Body of `PATCH /api/v1/plugins/repositories/{id}`.
#[derive(Deserialize, ToSchema)]
pub struct UpdatePluginRepositoryRequest {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Body of `PUT /api/v1/plugins/repositories/settings`.
#[derive(Deserialize, ToSchema)]
pub struct PluginRepositorySettingsRequest {
    /// Hours between two automatic refreshes, 1 to 168.
    pub refresh_hours: u32,
}

/// Who signed a package.
#[derive(Serialize, ToSchema)]
pub struct PluginPublisherResponse {
    pub key_id: String,
    /// Hex SHA-256 of the signing key.
    pub fingerprint: String,
    pub author: String,
}

impl From<Publisher> for PluginPublisherResponse {
    fn from(publisher: Publisher) -> Self {
        Self {
            key_id: publisher.key_id,
            fingerprint: publisher.fingerprint,
            author: publisher.author,
        }
    }
}

/// What a package asks for.
#[derive(Serialize, ToSchema)]
pub struct PluginPermissionsResponse {
    /// Capability names, `secrets:<reference>` and `net_stream:<ports>` included.
    pub granted: Vec<String>,
    pub http_domains: Vec<String>,
    pub stream_hosts: Vec<String>,
}

impl From<Permissions> for PluginPermissionsResponse {
    fn from(permissions: Permissions) -> Self {
        Self {
            granted: permissions.granted,
            http_domains: permissions.http_domains,
            stream_hosts: permissions.stream_hosts,
        }
    }
}

/// One package as an index lists it.
#[derive(Serialize, ToSchema)]
pub struct PluginIndexPackageResponse {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub api_version: String,
    pub min_app_version: Option<String>,
    pub package_digest: String,
    pub size: u64,
    pub publisher: PluginPublisherResponse,
    pub permissions: PluginPermissionsResponse,
    /// Plain text; shown as text, never as markup.
    pub release_notes: Option<String>,
}

impl From<IndexPackage> for PluginIndexPackageResponse {
    fn from(entry: IndexPackage) -> Self {
        Self {
            plugin_id: entry.id.to_string(),
            name: entry.name,
            version: entry.version,
            plugin_type: entry.plugin_type.as_str().to_owned(),
            api_version: entry.api_version,
            min_app_version: entry.min_app_version,
            package_digest: entry.package_digest,
            size: entry.size,
            publisher: entry.publisher.into(),
            permissions: entry.permissions.into(),
            release_notes: entry.release_notes,
        }
    }
}

/// One package an enabled repository offers.
#[derive(Serialize, ToSchema)]
pub struct PluginOfferResponse {
    pub repository_id: String,
    pub repository_name: String,
    pub official: bool,
    pub package: PluginIndexPackageResponse,
    /// `compatible`, `contract_unsupported`, `app_too_old` or `withdrawn`.
    pub compatibility: String,
    /// The newest version of this plugin installed here.
    pub installed_version: Option<String>,
}

impl From<Offer> for PluginOfferResponse {
    fn from(offer: Offer) -> Self {
        Self {
            repository_id: offer.repository_id,
            repository_name: offer.repository_name,
            official: offer.official,
            package: offer.entry.into(),
            compatibility: compatibility_name(offer.compatibility).to_owned(),
            installed_version: offer.installed_version,
        }
    }
}

/// A newer version of an installed plugin, from the key it is already signed with.
#[derive(Serialize, ToSchema)]
pub struct PluginUpdateResponse {
    pub offer: PluginOfferResponse,
    pub installed_version: String,
    /// `manual` or `automatic`.
    pub policy: String,
    /// Asks for a permission the installed version does not have; never installed
    /// automatically, whatever the policy.
    pub adds_permissions: bool,
    /// Those permissions, one list each; empty when `adds_permissions` is false (RD-160-09).
    pub added_permissions: PluginPermissionsResponse,
}

impl From<Update> for PluginUpdateResponse {
    fn from(update: Update) -> Self {
        Self {
            offer: update.offer.into(),
            installed_version: update.installed_version,
            policy: match update.policy {
                UpdatePolicy::Manual => "manual",
                UpdatePolicy::Automatic => "automatic",
            }
            .to_owned(),
            adds_permissions: update.adds_permissions,
            added_permissions: update.added_permissions.into(),
        }
    }
}

/// Updates for installed plugins, and what else the repositories offer.
#[derive(Serialize, ToSchema)]
pub struct PluginOffersResponse {
    pub updates: Vec<PluginUpdateResponse>,
    /// Packages of plugins not installed here.
    pub available: Vec<PluginOfferResponse>,
    /// Every package the repositories offer for plugins installed here, whatever its version:
    /// the release notes of each, for the version panel's history.
    pub installed: Vec<PluginOfferResponse>,
}

/// Which package of which repository.
#[derive(Deserialize, ToSchema)]
pub struct RepositoryPackageRequest {
    pub plugin_id: String,
    pub version: String,
    /// Install only: the fingerprint of the package's signing key the person confirmed.
    #[serde(default)]
    pub trust_fingerprint: Option<String>,
}

/// Where a previewed package comes from, when a repository offered it.
#[derive(Serialize, ToSchema)]
pub struct PluginPreviewSourceResponse {
    pub repository_id: String,
    pub repository_name: String,
    pub official: bool,
}

/// What a package asks for beyond the newest installed version of the same plugin (RD-160-09).
#[derive(Serialize, ToSchema)]
pub struct PluginAddedPermissionsResponse {
    /// The installed version the package is compared with.
    pub installed_version: String,
    /// Empty lists when the package asks for nothing new.
    pub permissions: PluginPermissionsResponse,
}

/// Everything the install preview shows. Nothing is installed by asking for it.
#[derive(Serialize, ToSchema)]
pub struct PluginPreviewResponse {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub api_version: String,
    pub min_app_version: Option<String>,
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub package_digest: String,
    pub size: u64,
    /// `None` for an unsigned package.
    pub publisher: Option<PluginPublisherResponse>,
    pub permissions: PluginPermissionsResponse,
    /// `trusted`, `untrusted`, `mismatch`, `withdrawn` or `unsigned`.
    pub key_status: String,
    /// Whether this exact package was withdrawn by its digest.
    pub withdrawn: bool,
    /// Why this build cannot run it, when it cannot.
    pub incompatible: Option<String>,
    /// Whether confirming can install it at all.
    pub installable: bool,
    /// Versions of the same plugin already installed here.
    pub installed_versions: Vec<String>,
    /// What the package asks for that the newest installed version does not; `None` when no
    /// version of the plugin is installed, so everything in `permissions` is new.
    pub added_permissions: Option<PluginAddedPermissionsResponse>,
    pub source: Option<PluginPreviewSourceResponse>,
    /// The index's notes for this version; a package on its own carries none.
    pub release_notes: Option<String>,
}

impl PluginPreviewResponse {
    /// `installed` is every installed version of the same plugin with what it asks for.
    pub(crate) fn new(preview: PackagePreview, installed: Vec<(String, Permissions)>) -> Self {
        let installable = preview.installable();
        let versions: Vec<String> = installed
            .iter()
            .map(|(version, _)| version.clone())
            .collect();
        // Held against the newest installed version, the one an update list compares with.
        let added_permissions = crate::plugin_lifecycle::effective(&versions, None)
            .and_then(|newest| {
                installed
                    .into_iter()
                    .find(|(version, _)| *version == newest)
            })
            .map(|(installed_version, held)| PluginAddedPermissionsResponse {
                installed_version,
                permissions: preview.permissions.beyond(&held).into(),
            });
        Self {
            plugin_id: preview.id.to_string(),
            name: preview.name,
            version: preview.version,
            plugin_type: preview.plugin_type.as_str().to_owned(),
            api_version: preview.api_version,
            min_app_version: preview.min_app_version,
            description: preview.description,
            homepage: preview.homepage,
            license: preview.license,
            package_digest: preview.package_digest,
            size: preview.size,
            publisher: preview.publisher.map(Into::into),
            permissions: preview.permissions.into(),
            key_status: match preview.key {
                KeyStatus::Trusted => "trusted",
                KeyStatus::Untrusted => "untrusted",
                KeyStatus::Mismatch => "mismatch",
                KeyStatus::Withdrawn => "withdrawn",
                KeyStatus::Unsigned => "unsigned",
            }
            .to_owned(),
            withdrawn: preview.withdrawn,
            incompatible: preview.incompatible,
            installable,
            installed_versions: versions,
            added_permissions,
            source: None,
            release_notes: None,
        }
    }

    /// Adds the repository and the release notes an offer carries.
    pub(crate) fn with_offer(mut self, offer: &Offer) -> Self {
        self.source = Some(PluginPreviewSourceResponse {
            repository_id: offer.repository_id.clone(),
            repository_name: offer.repository_name.clone(),
            official: offer.official,
        });
        self.release_notes = offer.entry.release_notes.clone();
        self
    }
}

fn compatibility_name(compatibility: PackageCompatibility) -> &'static str {
    match compatibility {
        PackageCompatibility::Compatible => "compatible",
        PackageCompatibility::ContractUnsupported => "contract_unsupported",
        PackageCompatibility::AppTooOld => "app_too_old",
        PackageCompatibility::Withdrawn => "withdrawn",
    }
}

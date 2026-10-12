//! What the bundled-plugin routes take and answer.

use super::*;

/// Query of `GET /api/v1/plugins/bundled`.
#[derive(Deserialize, IntoParams)]
pub struct BundledCatalogueQuery {
    /// Language of the names and descriptions (`de`, `en`, …); English when absent or not
    /// shipped by a package.
    #[serde(default)]
    pub locale: Option<String>,
}

/// Where a plugin comes from (RD-1260-02).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginOrigin {
    /// The packages shipped beside the program.
    Bundled,
    /// A plugin repository.
    Repository,
    /// A package somebody installed by hand.
    Upload,
}

/// One plugin of a bundled service.
#[derive(Serialize, ToSchema)]
pub struct BundledPluginResponse {
    pub id: PluginId,
    pub name: String,
    pub plugin_type: String,
    /// The version in the bundle; for a plugin only a repository offers, the version it offers.
    pub version: String,
    /// The newest installed version, `null` when the plugin is not installed.
    pub installed_version: Option<String>,
    /// Where an install takes the plugin from now (RD-1260-02): `repository` when a repository
    /// offers the bundle's version or a newer one of the same plugin, signed by the same key and
    /// asking for nothing more, and its host answers; `bundled` otherwise. In an install's answer,
    /// where the installed version came from.
    pub origin: PluginOrigin,
    /// The version an install from the repository brings, when `origin` is `repository`.
    pub repository_version: Option<String>,
    /// The repository that offers it, when `origin` is `repository`.
    pub repository_id: Option<String>,
    /// Where the installed version came from: `repository` when a repository delivered it,
    /// `bundled` when it is the bundle's version, `upload` otherwise; `null` when not installed.
    pub installed_origin: Option<PluginOrigin>,
    /// The update waiting for the installed version, as `GET /api/v1/plugins/updates` lists it.
    pub update: Option<crate::plugin_repository_dto::PluginUpdateResponse>,
}

/// How much of a service is installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundledServiceState {
    /// Every plugin of the service is installed.
    Installed,
    /// Some are: installing the service adds the rest.
    Partial,
    /// None is.
    Available,
}

/// One service of the bundle.
#[derive(Serialize, ToSchema)]
pub struct BundledServiceResponse {
    /// Stable key: the provider slug, or the plugin's own slug.
    pub key: String,
    pub name: String,
    pub description: String,
    /// `hoster`, `multihoster`, `remote_jobs`, `cloud`, `links`, `metadata`, `notifications`,
    /// `postprocess` or `other`.
    pub category: String,
    /// Whether the service does nothing until an account or a destination is set up. The ones
    /// that need none are what a fresh installation starts with.
    pub needs_account: bool,
    /// The provider an account for this service is created under, when it has one.
    pub provider: Option<String>,
    pub state: BundledServiceState,
    /// Whether a plugin of the service has an update waiting (RD-1260-02).
    pub update_available: bool,
    pub plugins: Vec<BundledPluginResponse>,
}

#[derive(Serialize, ToSchema)]
pub struct BundledCatalogueResponse {
    /// Ordered by category, then key. Empty when the service found no bundle directory.
    pub services: Vec<BundledServiceResponse>,
    /// What only the enabled repositories offer, one service per plugin with its id as the key
    /// (RD-1260-02): the newest version this build runs, installed or not. Installed through the
    /// install preview, never by `POST /api/v1/plugins/bundled/install`; the setup wizard does
    /// not list them.
    pub repository_services: Vec<BundledServiceResponse>,
}

/// Body of `POST /api/v1/plugins/bundled/install`.
#[derive(Deserialize, ToSchema)]
pub struct BundledInstallRequest {
    /// Keys of the services to install, as the catalogue lists them.
    pub services: Vec<String>,
}

/// One plugin that could not be installed, or removed.
#[derive(Serialize, ToSchema)]
pub struct BundledInstallFailure {
    pub service: String,
    pub plugin_id: PluginId,
    pub name: String,
    pub code: String,
    pub message: String,
}

/// Installs a service from the repository when it is reachable and the bundle otherwise
/// (RD-1260-02); each installed plugin's `origin` says which.
#[derive(Serialize, ToSchema)]
pub struct BundledInstallResponse {
    /// `plugin.bundled_installed` when everything installed runs now,
    /// `plugin.bundled_installed_restart_required` when some of it runs from the next start, or
    /// `plugin.bundled_partly_installed` when something failed.
    pub code: String,
    pub message: String,
    pub installed: Vec<BundledPluginResponse>,
    pub failed: Vec<BundledInstallFailure>,
    /// Whether a plugin installed here runs only from the next start (RD-170-12): an update of
    /// a running plugin, a type that is not loaded while the service runs, or one that did not
    /// load. The setup wizard's accounts step says so.
    pub restart_required: bool,
}

/// Body of `POST /api/v1/plugins/bundled/remove`.
#[derive(Deserialize, ToSchema)]
pub struct BundledRemoveRequest {
    /// Keys of the services to remove, as the catalogue lists them.
    pub services: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct BundledRemoveResponse {
    /// `plugin.bundled_removed` when every named service is gone, or
    /// `plugin.bundled_partly_removed` when something stayed.
    pub code: String,
    pub message: String,
    /// Keys of the services whose plugins were all removed.
    pub removed: Vec<String>,
    /// What stayed: a version an unfinished download is bound to (`plugin.version_in_use`)
    /// keeps its whole service, or a removal that failed.
    pub failed: Vec<BundledInstallFailure>,
}

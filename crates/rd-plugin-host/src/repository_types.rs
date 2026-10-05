//! The repository service's vocabulary: the update policy and where it comes from, the error
//! and its stable codes, a verified index held in memory, and what a probe found.
//!
//! Split out of `repository.rs` (PLUG-21).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::index::{IndexError, PluginIndex};

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
/// own store and hands it over with
/// [`PluginRepositoryService::set_update_policy`](super::PluginRepositoryService::set_update_policy).
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

pub(super) fn manual_updates() -> Arc<dyn UpdatePolicySource> {
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
